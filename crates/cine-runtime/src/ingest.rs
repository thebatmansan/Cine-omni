//! Bringing media into a project: one asset, and the work that follows it.
//!
//! Ingest is where a file becomes something the project can refer to. It does
//! the whole of that and nothing else:
//!
//! ```text
//! discover → identify → probe → evaluate → register → dispatch derived work
//! ```
//!
//! * **discover** — one file, or the numbered run it belongs to.
//! * **identify** — content, never a name: [`cine_media::sequence::identity_of`],
//!   the same function relinking and the proxies ask.
//! * **probe** — container, codecs, sizes, rates, durations, colour, sound.
//! * **evaluate** — what *this build* can do with what was found, from the
//!   capability registry rather than from hope.
//! * **register** — one transaction. Either the asset is in the project with
//!   everything the probe learned, or nothing is.
//! * **dispatch** — the derived work a person asked for, as jobs of its own.
//!
//! ## One asset, one identity
//!
//! The asset's id is derived from its content, so ingesting the same file twice
//! — or two members of the same run — is the same asset, and re-ingesting is
//! idempotent rather than a duplicate. Nothing here invents an identity;
//! proxies, waveforms, thumbnails, grades and renders all hang off this one.
//!
//! ## Cancelling
//!
//! Cancellation is honoured up to the moment of registration. Before it,
//! nothing has been written and the job answers `cancelled`; after it, the
//! asset exists and the job finishes, because a half-registered asset is the
//! state this refuses to leave behind. Derived work dispatched afterwards is
//! cancellable in its own right.

use cine_media::identity::MediaFile;
use cine_pipeline::OmniProject;
use serde_json::{json, Value as J};
use std::path::{Path, PathBuf};
use ucg_core::{EntityId, Rate, UcgError};

/// The job kind.
pub const INGEST_KIND: &str = "media.ingest";

/// What derived work an import asks for.
///
/// Thumbnails and waveforms are cheap and are what a browser needs to show
/// media at all, so they default to on where the media has that stream to make
/// them from. A proxy is minutes of work and gigabytes of disk, so it happens
/// only when asked for: no policy here decides that on a person's behalf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Derive {
    pub thumbnail: bool,
    pub waveform: bool,
    /// The proxy height to make, if any.
    pub proxy: Option<u32>,
}

impl Default for Derive {
    fn default() -> Self {
        Derive {
            thumbnail: true,
            waveform: true,
            proxy: None,
        }
    }
}

impl Derive {
    /// Nothing derived: what the bare `ingest` call does unless asked.
    ///
    /// The policy — thumbnails and waveforms by default — belongs to the
    /// import *job*, which is what a person uses. The primitive call makes an
    /// asset and nothing else, so a caller that wanted one asset does not
    /// silently get background work as well.
    pub const fn none() -> Derive {
        Derive {
            thumbnail: false,
            waveform: false,
            proxy: None,
        }
    }

    pub fn from_params(params: &J) -> Result<Derive, String> {
        Derive::from_params_or(params, Derive::default())
    }

    /// Read what was asked for, with `fallback` where nothing was said.
    pub fn from_params_or(params: &J, d: Derive) -> Result<Derive, String> {
        let flag =
            |key: &str, fallback: bool| params.get(key).and_then(J::as_bool).unwrap_or(fallback);
        let proxy = match params.get("proxy_height").and_then(J::as_u64) {
            None | Some(0) => None,
            Some(h) if (16..=4320).contains(&h) && h % 2 == 0 => Some(h as u32),
            Some(h) => return Err(format!("proxy height {h}: an even number from 16 to 4320")),
        };
        Ok(Derive {
            thumbnail: flag("thumbnail", d.thumbnail),
            waveform: flag("waveform", d.waveform),
            proxy,
        })
    }

    pub fn to_params(&self) -> J {
        json!({
            "thumbnail": self.thumbnail,
            "waveform": self.waveform,
            "proxy_height": self.proxy,
        })
    }
}

/// What to import.
#[derive(Clone, Debug)]
pub struct Request {
    pub path: PathBuf,
    /// `Some(true)` reads the numbered run, `Some(false)` one file, `None`
    /// lets the run decide: siblings make a sequence, one file is one still.
    pub sequence: Option<bool>,
    /// The rate a sequence is read at; a run of files has none of its own.
    pub rate: Rate,
    pub derive: Derive,
}

impl Request {
    pub fn from_params(params: &J) -> Result<Request, String> {
        Request::from_params_or(params, Derive::default())
    }

    /// Read a request, with `derive` where the caller said nothing about the
    /// work that should follow.
    pub fn from_params_or(params: &J, derive: Derive) -> Result<Request, String> {
        let path = PathBuf::from(
            params
                .get("path")
                .and_then(J::as_str)
                .ok_or("missing param `path`")?,
        );
        let rate = match (
            params.get("rate_num").and_then(J::as_u64),
            params.get("rate_den").and_then(J::as_u64),
        ) {
            (None, None) => cine_media::sequence::DEFAULT_RATE,
            (Some(n), Some(d)) => {
                Rate::new(n as u32, d as u32).ok_or("`rate_num`/`rate_den` must be a real rate")?
            }
            _ => return Err("`rate_num` and `rate_den` go together".into()),
        };
        Ok(Request {
            path,
            sequence: params.get("sequence").and_then(J::as_bool),
            rate,
            derive: Derive::from_params_or(params, derive)?,
        })
    }

    pub fn to_params(&self) -> J {
        let mut params = self.derive.to_params();
        params["path"] = json!(self.path.display().to_string());
        params["sequence"] = json!(self.sequence);
        params["rate_num"] = json!(self.rate.num());
        params["rate_den"] = json!(self.rate.den());
        params
    }
}

/// What an import produced.
#[derive(Clone, Debug)]
pub struct Ingested {
    pub media: EntityId,
    pub info: MediaFile,
    pub sequence: Option<cine_media::sequence::Sequence>,
    /// Jobs started for the derived work, by kind.
    pub dispatched: Vec<(String, u64)>,
    /// What could not be done, and why — a stream this build cannot decode, a
    /// derived job that could not be started. Never silence.
    pub notes: Vec<String>,
}

impl Ingested {
    pub fn to_json(&self) -> J {
        let video = self.info.first_video();
        let audio = self.info.first_audio();
        json!({
            "id": self.media,
            "sequence": self.sequence.is_some(),
            "pattern": self.sequence.as_ref().map(|s| s.pattern.clone()),
            "first": self.sequence.as_ref().map(|s| s.first),
            "last": self.sequence.as_ref().map(|s| s.last),
            "frames": self.sequence.as_ref().map(|s| s.frames()),
            "missing": self.sequence.as_ref().map(|s| s.missing.clone()),
            "rate_num": video.and_then(|s| s.video.as_ref()).and_then(|v| v.frame_rate).map(|r| r.num()),
            "rate_den": video.and_then(|s| s.video.as_ref()).and_then(|v| v.frame_rate).map(|r| r.den()),
            "content_hash": self.info.hash.hex(),
            "container": format!("{:?}", self.info.container),
            "picture": video.map(|s| json!({
                "codec": s.codec,
                "width": s.video.as_ref().map(|v| v.width),
                "height": s.video.as_ref().map(|v| v.height),
                "frames": s.video.as_ref().and_then(|v| v.frame_count),
                "decodable": s.decodable,
                "variable_frame_rate": s.video.as_ref().map(|v| v.variable_frame_rate),
            })),
            "sound": audio.map(|s| json!({
                "codec": s.codec,
                "sample_rate": s.audio.as_ref().map(|a| a.sample_rate),
                "channels": s.audio.as_ref().map(|a| a.channels),
                "bits": s.audio.as_ref().and_then(|a| a.bits_per_sample),
                "decodable": s.decodable,
            })),
            "dispatched": self.dispatched.iter().map(|(kind, id)| json!({ "kind": kind, "job": id })).collect::<Vec<_>>(),
            "notes": self.notes,
        })
    }
}

/// Find, identify and probe, without touching the project.
///
/// Separate from registering on purpose: everything that can fail about a file
/// fails here, before anything is written.
pub fn examine(
    request: &Request,
) -> Result<(MediaFile, Option<cine_media::sequence::Sequence>), String> {
    if !request.path.exists() {
        return Err(format!("no file at {}", request.path.display()));
    }
    let run = match request.sequence {
        Some(false) => None,
        _ => cine_media::sequence::discover(&request.path).ok(),
    };
    let run = match (request.sequence, run) {
        (Some(true), None) => {
            return Err(format!(
                "{} is not one of a numbered run of pictures",
                request.path.display()
            ))
        }
        // On its own, a numbered file is one still: a run needs more than one
        // member before it is a sequence.
        (None, Some(seq)) if seq.frames() < 2 => None,
        (_, other) => other,
    };
    let info = match &run {
        Some(_) => cine_media::sequence::probe_sequence(&request.path, request.rate)
            .map_err(|e| e.to_string())?,
        None => cine_media::probe::probe(&request.path).map_err(|e| e.to_string())?,
    };
    Ok((info, run))
}

/// What this build can do with what was found, in the words of the registry.
pub fn evaluate(info: &MediaFile) -> Vec<String> {
    let mut notes = Vec::new();
    for stream in &info.streams {
        if stream.decodable {
            continue;
        }
        match cine_media::capability::refusal_reason(&stream.codec, None) {
            Some(reason) => notes.push(format!(
                "{} {}: {reason}",
                stream.kind.as_str(),
                stream.codec
            )),
            None => notes.push(format!(
                "{} {}: this build has no decoder for it, so it cannot be played or rendered",
                stream.kind.as_str(),
                stream.codec
            )),
        }
    }
    if info.streams.is_empty() {
        notes.push("the file declares no streams this build understands".into());
    }
    notes
}

/// Import one file: examine it, register it, and start the derived work.
///
/// `progress` reports the stage and returns `false` to cancel; cancelling is
/// honoured until the asset is registered.
pub fn ingest(
    project: &OmniProject,
    request: &Request,
    progress: &mut dyn FnMut(f32, &str) -> bool,
) -> Result<Option<Ingested>, String> {
    if !progress(0.05, "looking at the file") {
        return Ok(None);
    }
    let (info, sequence) = examine(request)?;
    if !progress(0.4, "identifying and probing") {
        return Ok(None);
    }
    let mut notes = evaluate(&info);

    if !progress(0.6, "registering the asset") {
        return Ok(None);
    }
    let root = project.root;
    let mut media = EntityId::nil();
    project
        .engine
        .transact("ingest media", |tx| {
            media = cine_media::ucg::register_media(tx, root, &info)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            Ok(())
        })
        .map_err(|e| e.to_string())?;

    // From here the asset exists, so the import is done whatever else happens;
    // the derived work is jobs of its own, each cancellable and retryable.
    progress(0.8, "starting the derived work");
    let dispatched = dispatch(project, media, &info, request.derive, &mut notes);
    progress(1.0, "imported");
    Ok(Some(Ingested {
        media,
        info,
        sequence,
        dispatched,
        notes,
    }))
}

/// Start the derived work this import asked for, where the media has what it
/// needs. Anything that cannot be started is a note, not a failure: the asset
/// is real either way.
fn dispatch(
    project: &OmniProject,
    media: EntityId,
    info: &MediaFile,
    derive: Derive,
    notes: &mut Vec<String>,
) -> Vec<(String, u64)> {
    let Some(jobs) = project.jobs.get() else {
        if derive.thumbnail || derive.waveform || derive.proxy.is_some() {
            notes.push(
                "no background work is running in this process, so nothing derived was started"
                    .into(),
            );
        }
        return Vec::new();
    };
    let picture = info.first_video().filter(|s| s.decodable).is_some();
    let sound = info.first_audio().filter(|s| s.decodable).is_some();
    let mut started = Vec::new();
    let mut start = |kind: &str, params: J, notes: &mut Vec<String>| match crate::jobs::spec_for(
        project, kind, &params,
    ) {
        Ok(spec) => started.push((kind.to_string(), jobs.submit(spec))),
        Err(why) => notes.push(format!("{kind} was not started: {why}")),
    };
    if derive.thumbnail {
        if picture {
            start(
                "media.thumbnail",
                json!({ "media": media, "index": 0 }),
                notes,
            );
        } else {
            notes.push("no thumbnail: this media has no picture this build can decode".into());
        }
    }
    if derive.waveform {
        if sound {
            start(
                crate::waveform::WAVEFORM_KIND,
                json!({ "media": media }),
                notes,
            );
        } else {
            notes.push("no waveform: this media has no sound this build can decode".into());
        }
    }
    if let Some(height) = derive.proxy {
        if picture {
            start(
                crate::proxy::PROXY_KIND,
                json!({ "media": media, "height": height }),
                notes,
            );
        } else {
            notes.push("no proxy: this media has no picture this build can decode".into());
        }
    }
    started
}

/// A label for the job list: what is being imported.
pub fn label(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    format!("Import · {name}")
}
