//! Proxies: smaller stand-ins for camera originals, made as jobs, used by
//! choice, and never mistaken for the original.
//!
//! ## What a proxy is in the project
//!
//! A proxy is a `media.derivative` node with a `DerivedFrom` edge to the
//! original's `asset.media` node — never an asset of its own, so no clip can
//! bind to it and no render can read it. It records what it was made from (the
//! original's content hash), how (a recipe: codec, size, quality, and this
//! module's version), what it is (its own content hash, size on disk, probed
//! streams), and whether it carries sound.
//!
//! A proxy has the original's frame count and frame rate, picture for picture,
//! so picture `i` of the proxy is picture `i` of the original and every edit
//! resolves to the same instant in either. Its sound, when it has any, starts
//! at the original's first picture, so a clip's sound is read from the same
//! offset in either file.
//!
//! ## Current, stale, missing
//!
//! * **missing** — its file is gone.
//! * **stale** — the original it was made from is not the original the project
//!   has (another content hash), it was made by an older recipe, or its own file
//!   changed on disk since it was made.
//! * **current** — none of those.
//!
//! Playback never reads a proxy that is not current, and says why when it
//! reads the original instead.
//!
//! ## Which file plays
//!
//! [`choose`] is the one decision, used by every surface that plays:
//!
//! * **Original** — the original.
//! * **Proxy** — the best current proxy (the largest), otherwise the original
//!   with the reason.
//! * **Auto** — a current proxy when the original is offline, cannot be
//!   decoded by this build, or is larger than HD ([`AUTO_PIXELS`]); otherwise
//!   the original.
//!
//! Sound follows picture only when the proxy carries sound; a proxy made
//! without it (a source with more than two channels, which has no downmix)
//! leaves the clip's sound on the original, and says so.

use cine_comp::image::{ChannelSet, Image};
use cine_comp::ops::resample;
use cine_media::encode::{H264Settings, YuvFrame};
use cine_media::identity::{Derivative, DerivativeKind};
use cine_media::mov::{ColourTags, MovSettings, MovWriter, SoundFormat};
use cine_media::sequence_audio::{SequenceAudio, SoundEntry};
use cine_media::AudioSource;
use cine_pipeline::session::MediaPreference;
use cine_pipeline::OmniProject;
use serde_json::{json, Value as J};
use std::path::{Path, PathBuf};
use std::time::Instant;
use ucg_core::{EntityId, Graph, Rate, RationalTime, TimeRange, UcgError};
use ucg_schema::{attrs, kinds};

/// The job kind.
pub const PROXY_KIND: &str = "media.proxy";

/// Bumped when the way proxies are made changes what they contain; proxies
/// made by another version are stale.
pub const RECIPE_VERSION: &str = "proxy/1";

/// Auto plays a current proxy for originals with more pixels than this.
pub const AUTO_PIXELS: u64 = 1920 * 1080;

/// Sound in a proxy: this rate, 24-bit, the original's one or two channels.
pub const SOUND_RATE: u32 = 48_000;

pub use crate::derived::{FILE_SIZE_ATTR, JOB_ATTR, NOTES_ATTR, SOURCE_HASH_ATTR};

pub const WIDTH_ATTR: &str = "proxy_width";
pub const HEIGHT_ATTR: &str = "proxy_height";
pub const SOUND_ATTR: &str = "proxy_sound";

/// How a proxy is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProxySettings {
    /// Picture height; the width follows the original's shape, rounded to an
    /// even number.
    pub height: u32,
    /// H.264 quantiser, 0 (best) to 51.
    pub quality: u8,
}

impl Default for ProxySettings {
    fn default() -> Self {
        ProxySettings {
            height: 540,
            quality: 23,
        }
    }
}

impl ProxySettings {
    pub fn from_params(params: &J) -> Result<ProxySettings, String> {
        let d = ProxySettings::default();
        let height = match params.get("height").and_then(J::as_u64) {
            None => d.height,
            Some(h) if (16..=4320).contains(&h) && h % 2 == 0 => h as u32,
            Some(h) => return Err(format!("proxy height {h}: an even number from 16 to 4320")),
        };
        let quality = match params.get("quality").and_then(J::as_u64) {
            None => d.quality,
            Some(q) if q <= 51 => q as u8,
            Some(q) => return Err(format!("quality {q}: H.264 quantisers run 0 to 51")),
        };
        Ok(ProxySettings { height, quality })
    }

    /// The proxy size for an original of `w × h`, or why there is none.
    pub fn size_for(&self, w: u32, h: u32) -> Result<(u32, u32), String> {
        if h == 0 || w == 0 {
            return Err("the original declares no picture size".into());
        }
        if self.height >= h {
            return Err(format!(
                "a {}-line proxy of a {w}x{h} original would not be smaller than it",
                self.height
            ));
        }
        let width = ((u64::from(w) * u64::from(self.height) + u64::from(h) / 2) / u64::from(h)) as u32;
        let width = (width + 1) & !1;
        Ok((width.max(2), self.height))
    }

    pub fn recipe(&self, w: u32, h: u32) -> String {
        format!("h264-baseline qp{} {w}x{h} pcm-s24le-48k {RECIPE_VERSION}", self.quality)
    }
}

/// Where proxies of a file with content hash `hash` live.
pub fn proxy_root(root: &Path, hash: &str) -> PathBuf {
    let prefix: String = hash.chars().take(16).collect();
    root.join("derived").join("proxy").join(prefix)
}

fn recipe_dir(root: &Path, hash: &str, recipe: &str) -> PathBuf {
    use sha1::{Digest, Sha1};
    let key: String = Sha1::digest(recipe.as_bytes())
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect();
    proxy_root(root, hash).join(key)
}

/// The proxies recorded for an original.
pub fn proxies_of(graph: &Graph, media: EntityId) -> Vec<EntityId> {
    crate::derived::of_kind(graph, media, "proxy")
}

/// Whether a proxy may stand in for its original: [`crate::derived::status`],
/// under the proxy recipe's version.
pub type ProxyStatus = crate::derived::Status;

/// The state of `proxy`. `deep` also re-hashes the proxy file.
pub fn status(graph: &Graph, proxy: EntityId, deep: bool) -> ProxyStatus {
    crate::derived::status(graph, proxy, RECIPE_VERSION, deep)
}

/// Which file a clip reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub path: Option<PathBuf>,
    /// The proxy read, when one is.
    pub proxy: Option<EntityId>,
    /// Whether the chosen file is there.
    pub online: Option<bool>,
    /// Whether the chosen file carries the clip's sound.
    pub has_sound: bool,
    /// Why this file and not the other, when that is not the obvious answer.
    pub reason: Option<String>,
}

/// The file `media` is read from under `preference`. See the module notes.
pub fn choose(graph: &Graph, media: EntityId, preference: MediaPreference) -> Choice {
    let node = graph.node(media);
    let original = Choice {
        path: node
            .and_then(|n| n.get_str(attrs::MEDIA_PATH))
            .map(PathBuf::from),
        proxy: None,
        online: node.and_then(|n| n.get_bool(cine_media::ucg::ONLINE_ATTR)),
        has_sound: true,
        reason: None,
    };
    if preference == MediaPreference::Original {
        return original;
    }
    // The best current proxy: the largest.
    let mut stale: Option<String> = None;
    let mut best: Option<(u32, EntityId)> = None;
    for p in proxies_of(graph, media) {
        let st = status(graph, p, false);
        if st.state == "current" {
            let h = graph
                .node(p)
                .and_then(|n| n.get_int(HEIGHT_ATTR))
                .unwrap_or(0) as u32;
            if best.is_none_or(|(bh, _)| h > bh) {
                best = Some((h, p));
            }
        } else if stale.is_none() {
            stale = Some(format!("its proxy is {}: {}", st.state, st.reasons.join("; ")));
        }
    }
    let proxy_choice = |p: EntityId, reason: Option<String>| {
        let n = graph.node(p);
        Choice {
            path: n.and_then(|n| n.get_str(attrs::MEDIA_PATH)).map(PathBuf::from),
            proxy: Some(p),
            online: Some(true),
            has_sound: n.and_then(|n| n.get_bool(SOUND_ATTR)).unwrap_or(false),
            reason,
        }
    };
    let no_proxy = || stale.clone().unwrap_or_else(|| "it has no proxy".into());
    match preference {
        MediaPreference::Original => unreachable!("returned above"),
        MediaPreference::Proxy => match best {
            Some((_, p)) => proxy_choice(p, None),
            None => Choice {
                reason: Some(format!("playing the original: {}", no_proxy())),
                ..original
            },
        },
        MediaPreference::Auto => {
            let info = cine_media::ucg::read_media(graph, media);
            let video = info.as_ref().and_then(|i| i.first_video().cloned());
            let why = if original.online == Some(false) {
                Some("the original is offline".to_string())
            } else if video.as_ref().is_some_and(|v| !v.decodable) {
                Some(format!(
                    "this build cannot decode the original's {}",
                    video.as_ref().map(|v| v.codec.as_str()).unwrap_or("picture")
                ))
            } else {
                video.and_then(|v| v.video).and_then(|v| {
                    (u64::from(v.width) * u64::from(v.height) > AUTO_PIXELS).then(|| {
                        format!("the original is {}x{}, larger than HD", v.width, v.height)
                    })
                })
            };
            match (why, best) {
                (Some(why), Some((_, p))) => proxy_choice(p, Some(format!("playing the proxy: {why}"))),
                (Some(why), None) => Choice {
                    reason: Some(format!("playing the original although {why}: {}", no_proxy())),
                    ..original
                },
                (None, _) => original,
            }
        }
    }
}

/// Everything about an original's proxies, for the application.
pub fn proxies_json(graph: &Graph, media: EntityId) -> J {
    J::Array(
        proxies_of(graph, media)
            .into_iter()
            .filter_map(|p| {
                let n = graph.node(p)?;
                let st = status(graph, p, true);
                Some(json!({
                    "id": p,
                    "state": st.state,
                    "reasons": st.reasons,
                    "path": n.get_str(attrs::MEDIA_PATH),
                    "width": n.get_int(WIDTH_ATTR),
                    "height": n.get_int(HEIGHT_ATTR),
                    "sound": n.get_bool(SOUND_ATTR),
                    "recipe": n.get_str("recipe"),
                    "file_size": n.get_int(FILE_SIZE_ATTR),
                    "job": n.get_int(JOB_ATTR),
                    "notes": n.get(NOTES_ATTR).map(|v| v.to_json()),
                }))
            })
            .collect(),
    )
}

/// Make a proxy of `media` under `root`. `progress` returns `false` to cancel.
pub fn generate(
    project: &OmniProject,
    root: &Path,
    media: EntityId,
    settings: ProxySettings,
    job: Option<u64>,
    progress: &mut dyn FnMut(f32, &str) -> bool,
) -> Result<J, String> {
    let began = Instant::now();
    let (path, hash, info, name) = {
        let g = project.engine.read();
        let n = g.node(media).ok_or("the media is no longer in the project")?;
        if n.kind.as_str() != kinds::ASSET_MEDIA {
            return Err(format!("{media} is not a camera original; proxies are made of originals"));
        }
        let path = n
            .get_str(attrs::MEDIA_PATH)
            .map(PathBuf::from)
            .ok_or("the media has no path")?;
        let hash = n
            .get_str(cine_media::ucg::HASH_ATTR)
            .ok_or("the media has no content hash")?
            .to_string();
        let info = cine_media::ucg::read_media(&g, media).ok_or("the media was never probed")?;
        (path, hash, info, n.name().to_string())
    };
    if !path.is_file() {
        return Err(format!("the original is offline: {}", path.display()));
    }
    let now = cine_media::sequence::identity_of(&info).map_err(|e| e.to_string())?;
    if now.hex() != hash {
        return Err(format!(
            "{} changed on disk since it was ingested; ingest it again before making a proxy of it",
            path.display()
        ));
    }
    let stream = info
        .first_video()
        .cloned()
        .ok_or("the original has no picture to make a proxy of")?;
    if !stream.decodable {
        return Err(format!(
            "this build cannot decode the original's {} picture, so it cannot make a proxy of it",
            stream.codec
        ));
    }
    let v = stream.video.ok_or("the picture stream declares no size")?;
    let rate: Rate = match (v.frame_rate, v.variable_frame_rate) {
        (Some(r), false) => r,
        _ => {
            return Err(
                "the original's frame rate is variable or undeclared; a proxy must match it picture for picture"
                    .into(),
            )
        }
    };
    let (pw, ph) = settings.size_for(v.width, v.height)?;
    let recipe = settings.recipe(pw, ph);
    let out = recipe_dir(root, &hash, &recipe);
    let derivative = Derivative {
        kind: DerivativeKind::Proxy,
        source_hash: hash.clone(),
        path: out.join("proxy.mov"),
        recipe: recipe.clone(),
    };
    let id = derivative.entity_id();
    {
        let g = project.engine.read();
        if g.node(id).is_some() && status(&g, id, true).state == "current" {
            return Ok(json!({ "proxy": id, "already_current": true, "path": derivative.path }));
        }
    }

    let mut reader = cine_media::Pictures::open(info.clone()).map_err(|e| e.to_string())?;
    // A proxy is 8-bit video, and it stands in for its original wherever that
    // original plays. Baking a view into it would make it display-referred
    // media substituting for scene-linear media, and the window would then
    // apply the view a second time. So scene-linear originals have no proxy,
    // and the reason is the one that matters.
    if reader.is_scene_float() {
        return Err(format!(
            "no proxy of {}: its pictures are scene-linear, and an 8-bit stand-in would have to \
             bake in a view — which would then be applied twice wherever it plays",
            reader.describe()
        ));
    }
    let count = reader.frame_count();
    if count == 0 {
        return Err("the original has no pictures".into());
    }
    let mut notes: Vec<String> = Vec::new();
    // Sound: the original's, from its first picture, when it has one or two
    // channels. More have no downmix, so the proxy carries none and playback
    // keeps the clip's sound on the original.
    let mut sound = match info.first_audio().and_then(|s| s.audio.map(|a| (s, a))) {
        None => None,
        Some((s, _)) if !s.decodable => {
            notes.push(format!("no sound: this build cannot decode the original's {}", s.codec));
            None
        }
        Some((_, a)) if a.channels > 2 => {
            notes.push(format!(
                "no sound: the original has {} channels and there is no downmix; its sound plays from the original",
                a.channels
            ));
            None
        }
        Some((_, a)) => {
            if a.sample_rate != SOUND_RATE {
                notes.push(format!(
                    "sound converted from {} Hz to {SOUND_RATE} Hz by the player's kernel",
                    a.sample_rate
                ));
            }
            let end = RationalTime::exact(count as i64, rate);
            let mut mixer = SequenceAudio::new(
                Rate::whole(SOUND_RATE).expect("48 kHz"),
                end,
                vec![SoundEntry {
                    record: TimeRange::from_start_end(RationalTime::zero(rate), end),
                    path: Some(path.clone()),
                    offset: Some(RationalTime::zero(rate)),
                    name: name.clone(),
                    map: None,
                    keep_pitch: false,
                }],
            );
            match mixer.open_all().values().next() {
                Some(why) => {
                    notes.push(format!("no sound: {why}"));
                    None
                }
                None => Some(mixer),
            }
        }
    };

    let partial = {
        let mut n = out.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        n.push(".partial");
        out.with_file_name(n)
    };
    let _ = std::fs::remove_dir_all(&partial);
    let mut stages = J::Null;
    let result = (|| -> Result<Option<cine_media::mov::MovReport>, String> {
        std::fs::create_dir_all(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        let mut writer: Option<MovWriter> = None;
        let conversion = cine_gpu::yuv::YuvConversion {
            choice: cine_gpu::yuv::MatrixChoice::Declared {
                code: 1,
                matrix: cine_gpu::yuv::Matrix::BT709,
            },
            range: cine_gpu::yuv::Range::Limited,
            width: pw,
            height: ph,
            chroma_width: pw / 2,
            chroma_height: ph / 2,
        };
        let mut sound_buf = Vec::new();
        let mut sound_pos = 0i64;
        // Where the time goes, per stage, so a slow proxy is diagnosable
        // rather than merely slow.
        let (mut decode, mut convert, mut scale, mut encode) = (0u128, 0u128, 0u128, 0u128);
        for i in 0..count {
            if !progress(i as f32 / count as f32, &format!("picture {i} of {count}")) {
                return Ok(None);
            }
            let t0 = Instant::now();
            let frame = reader
                .frame_at_index(i)
                .map_err(|e| format!("picture {i}: {e}"))?
                .ok_or_else(|| format!("the original has no picture {i}"))?;
            decode += t0.elapsed().as_micros();
            if writer.is_none() {
                let colour = match frame.color.primaries.zip(frame.color.transfer) {
                    Some((p, t)) => ColourTags {
                        primaries: u16::from(p),
                        transfer: u16::from(t),
                    },
                    None => {
                        notes.push("primaries and transfer undeclared by the original; declared unspecified".into());
                        ColourTags::UNSPECIFIED
                    }
                };
                writer = Some(
                    MovWriter::create(
                        &partial.join("proxy.mov"),
                        pw as usize,
                        ph as usize,
                        rate,
                        MovSettings {
                            h264: H264Settings {
                                qp: settings.quality,
                                gop: 24,
                            },
                            sound: sound.as_ref().map(|_| SoundFormat {
                                sample_rate: SOUND_RATE,
                                channels: 2,
                            }),
                            colour,
                            first_picture: 0,
                        },
                    )
                    .map_err(|e| e.to_string())?,
                );
            }
            let t0 = Instant::now();
            let (px, fw, fh, describe, assumed) = crate::frames::rgba_f32(&frame)?;
            convert += t0.elapsed().as_micros();
            if assumed && i == 0 {
                notes.push(format!("decoded as {describe}, which the original does not declare"));
            }
            let img = Image::from_data(fw as usize, fh as usize, ChannelSet::Rgba, px)
                .ok_or("picture buffer has the wrong size")?;
            let t0 = Instant::now();
            let small = resample::resize(&img, pw as usize, ph as usize).map_err(|e| e.to_string())?;
            scale += t0.elapsed().as_micros();
            let t0 = Instant::now();
            let ([y, u, v], _) = cine_gpu::yuv::rgb_to_yuv8_cpu(&conversion, &small.data);
            let w = writer.as_mut().expect("created above");
            sound_buf.clear();
            if let Some(mixer) = sound.as_mut() {
                let due = w.sound_frames_due() as usize;
                mixer
                    .read(sound_pos, due, &mut sound_buf)
                    .map_err(|e| format!("sound at picture {i}: {e}"))?;
                sound_buf.resize(due * 2, 0.0);
                sound_pos += due as i64;
            }
            w.write_frame(
                &YuvFrame {
                    width: pw as usize,
                    height: ph as usize,
                    y,
                    u,
                    v,
                },
                &sound_buf,
            )
            .map_err(|e| format!("picture {i}: {e}"))?;
            encode += t0.elapsed().as_micros();
        }
        stages = json!({
            "decode_ms": decode as f64 / 1000.0,
            "yuv_to_rgb_ms": convert as f64 / 1000.0,
            "scale_ms": scale as f64 / 1000.0,
            "rgb_to_yuv_and_encode_ms": encode as f64 / 1000.0,
        });
        let report = writer
            .ok_or("no picture was written")?
            .finish()
            .map_err(|e| e.to_string())?;
        if report.frames != count as u64 {
            return Err(format!(
                "wrote {} pictures of the original's {count}",
                report.frames
            ));
        }
        Ok(Some(report))
    })();

    let report = match result {
        Ok(Some(r)) => r,
        Ok(None) => {
            discard(&partial);
            return Ok(json!({ "cancelled": true }));
        }
        Err(e) => {
            discard(&partial);
            return Err(e);
        }
    };

    let manifest = json!({
        "kind": PROXY_KIND,
        "original": { "media": media, "name": name, "path": path, "content_hash": hash },
        "recipe": recipe,
        "width": pw,
        "height": ph,
        "frames": count,
        "rate_num": rate.num(),
        "rate_den": rate.den(),
        "sound": sound.is_some(),
        "notes": notes,
        "job": job,
        "stages": stages,
        "bytes": report.file_bytes,
        "elapsed_ms": began.elapsed().as_millis() as u64,
    });
    std::fs::write(
        partial.join("manifest.json"),
        serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    // Replacing a stale proxy of the same recipe is what was asked for: the
    // new one is complete before the old one goes.
    if out.exists() {
        let mut old = out.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        old.push(".replaced");
        let old = out.with_file_name(old);
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(&out, &old).map_err(|e| e.to_string())?;
        std::fs::rename(&partial, &out).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir_all(&old);
    } else {
        std::fs::rename(&partial, &out).map_err(|e| e.to_string())?;
    }

    let file = out.join("proxy.mov");
    let probed = cine_media::probe::probe(&file).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    project
        .engine
        .transact("make proxy", |tx| {
            let id = cine_media::ucg::register_derivative(tx, media, &derivative)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            tx.set(id, attrs::NAME, format!("{name} · proxy {pw}x{ph}"))?;
            tx.set(id, SOURCE_HASH_ATTR, hash.as_str())?;
            tx.set(id, cine_media::ucg::HASH_ATTR, probed.hash.hex())?;
            tx.set(id, WIDTH_ATTR, i64::from(pw))?;
            tx.set(id, HEIGHT_ATTR, i64::from(ph))?;
            tx.set(id, SOUND_ATTR, sound.is_some())?;
            tx.set(id, FILE_SIZE_ATTR, size as i64)?;
            tx.set(id, NOTES_ATTR, ucg_core::Value::from_serde(&notes).map_err(|e| UcgError::Aborted(e.to_string()))?)?;
            tx.set(
                id,
                cine_media::ucg::MEDIA_INFO_ATTR,
                ucg_core::Value::from_serde(&probed).map_err(|e| UcgError::Aborted(e.to_string()))?,
            )?;
            if let Some(job) = job {
                tx.set(id, JOB_ATTR, job as i64)?;
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    let elapsed = began.elapsed().as_secs_f64();
    Ok(json!({
        "proxy": id,
        "path": file,
        "width": pw,
        "height": ph,
        "frames": count,
        "sound": sound.is_some(),
        "notes": notes,
        "bytes": size,
        "stages": stages,
        "elapsed_ms": (elapsed * 1000.0) as u64,
        "pictures_per_second": count as f64 / elapsed.max(1e-9),
    }))
}

/// Remove a partial directory — temporary data — and the empty directories
/// above it that only it needed. `remove_dir` removes only empty directories.
fn discard(partial: &Path) {
    let _ = std::fs::remove_dir_all(partial);
    let mut parent = partial.parent();
    for _ in 0..2 {
        match parent {
            Some(p) if std::fs::remove_dir(p).is_ok() => parent = p.parent(),
            _ => break,
        }
    }
}
