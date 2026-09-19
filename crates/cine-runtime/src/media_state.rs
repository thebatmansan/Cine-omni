//! What the project knows about one media, in one place.
//!
//! Five surfaces used to decide this for themselves — the project browser, the
//! inspector's summary, the inspector's resolution section, the record
//! monitor and the frame server — and each wrote its own words from the same
//! one flag. The flag is a tri-state pretending to be a boolean: `true` set
//! when media was ingested, `false` set when a check found it gone, and
//! **absent** for media that was never probed at all, which is what a clip
//! imported from an OTIO or an EDL is. Every surface tested `online !== false`,
//! so *never established* read as *online*, and a take nobody had ever
//! looked at was reported as present at its recorded path.
//!
//! Two things follow, and both are the point of this module.
//!
//! **Presence is not one bit.** It is *what* the project believes, and *how it
//! came to believe it*. Those are separate questions, and collapsing them is
//! how a fact recorded at import time comes to be shown as a fact about now.
//! [`Presence`] answers the first and [`Evidence`] the second, and neither is
//! reported without the other.
//!
//! **Nothing here touches the disk.** A projection is rebuilt on every change
//! and a project may hold thousands of assets; stat-ing all of them on every
//! keystroke would make the interface a filesystem crawler. This reads what
//! the project recorded and says when it was recorded. Establishing presence
//! afresh is what `check_media` is for, and it is a job a person asks for.

use cine_media::locator::Locator;
use cine_media::ucg::ONLINE_ATTR;
use serde_json::{json, Value as J};
use ucg_core::{EntityId, Graph};
use ucg_schema::attrs;

/// When the project last established where a media is, written by whatever
/// established it. Seconds since the epoch.
pub const CHECKED_AT_ATTR: &str = "presence_checked_at";
/// What established it: `import` or `check`.
pub const CHECKED_BY_ATTR: &str = "presence_checked_by";

/// What the project believes about a media's availability.
///
/// Ordered by how much is known, least first. Nothing collapses into "not
/// found": a node that is missing from the project and a node whose file is
/// missing from the disk are different facts needing different work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Presence {
    /// **A.** No node with that id. The only honest "not found".
    NoSuchMedia,
    /// **C.** Registered, never probed — a reference from an imported cut.
    /// Nothing is known about what is in the file, or whether there is one.
    NeverProbed,
    /// **B.** Probed, and the project records it as present where it says.
    Present,
    /// **D.** Probed, and a check found nothing there.
    Offline,
    /// **E.** A different file is at the recorded location.
    Replaced,
    /// **F.** A scan could not find this content anywhere it looked.
    NotFoundByContent,
    /// **G.** The file is readable and nothing in this build decodes it.
    Undecodable,
    /// **H.** Recorded somewhere this machine cannot open at all — a remote
    /// URL, a volume, a scheme with no meaning here.
    NotOnThisMachine,
}

impl Presence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Presence::NoSuchMedia => "no_such_media",
            Presence::NeverProbed => "never_probed",
            Presence::Present => "present",
            Presence::Offline => "offline",
            Presence::Replaced => "replaced",
            Presence::NotFoundByContent => "not_found_by_content",
            Presence::Undecodable => "undecodable",
            Presence::NotOnThisMachine => "not_on_this_machine",
        }
    }

    /// Whether a picture may be read from this media.
    pub fn is_readable(&self) -> bool {
        matches!(self, Presence::Present)
    }

    /// A short label, in the words a cutting room uses.
    pub fn label(&self) -> &'static str {
        match self {
            Presence::NoSuchMedia => "Not in this project",
            Presence::NeverProbed => "Not yet verified",
            Presence::Present => "Online",
            Presence::Offline => "Offline",
            Presence::Replaced => "Different file at this path",
            Presence::NotFoundByContent => "Not found by content",
            Presence::Undecodable => "Cannot be decoded",
            Presence::NotOnThisMachine => "Not on this machine",
        }
    }
}

/// How the project came to believe what it believes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// Nobody has ever established it.
    Never,
    /// Recorded when the media was imported, and not confirmed since.
    AtImport,
    /// Established by a presence check.
    Checked,
}

impl Evidence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Evidence::Never => "never",
            Evidence::AtImport => "at_import",
            Evidence::Checked => "checked",
        }
    }
}

/// Everything the project knows about one media's availability.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaState {
    pub presence: Presence,
    pub evidence: Evidence,
    /// Seconds since the epoch, when the evidence was established.
    pub at: Option<i64>,
    /// The location as recorded, parsed into what it actually is.
    pub locator: Locator,
    /// Why, in a sentence, when the state needs one.
    pub detail: Option<String>,
    /// The operation that would resolve this, when one would.
    pub action: Option<&'static str>,
}

impl MediaState {
    /// What the project knows about `media`.
    pub fn of(graph: &Graph, media: EntityId) -> MediaState {
        let info = cine_media::ucg::read_media(graph, media);
        MediaState::of_with(graph, media, info.as_ref())
    }

    /// The same, for a caller that has already read the media's description.
    ///
    /// The media projection has it in hand; reading it again would deserialise
    /// every asset's probe a second and third time on every change to the
    /// project, which for a real card of takes is the interface spending its
    /// time re-parsing what it just parsed.
    pub fn of_with(
        graph: &Graph,
        media: EntityId,
        info: Option<&cine_media::identity::MediaFile>,
    ) -> MediaState {
        let Some(node) = graph.node(media) else {
            return MediaState {
                presence: Presence::NoSuchMedia,
                evidence: Evidence::Never,
                at: None,
                locator: Locator::Nothing,
                detail: Some("no media with that id is in this project".into()),
                action: None,
            };
        };
        let locator = Locator::parse(node.get_str(attrs::MEDIA_PATH).unwrap_or_default());
        let at = node.get_int(CHECKED_AT_ATTR);
        let evidence = match node.get_str(CHECKED_BY_ATTR) {
            Some("check") => Evidence::Checked,
            Some("import") => Evidence::AtImport,
            // Written before presence carried its provenance: the project
            // knows a state and not where it came from, and says exactly that.
            _ if node.get_bool(ONLINE_ATTR).is_some() => Evidence::AtImport,
            _ => Evidence::Never,
        };
        let probed = info.is_some();
        let relink_state = node.get_str(crate::relink::STATE_ATTR);
        let note = node.get_str(crate::relink::NOTE_ATTR).map(str::to_string);

        let (presence, detail, action) = match (relink_state, probed, node.get_bool(ONLINE_ATTR)) {
            (Some("replaced"), _, _) => (Presence::Replaced, note.clone(), Some("relink")),
            (Some("not_found"), _, _) => {
                (Presence::NotFoundByContent, note.clone(), Some("relink"))
            }
            // Nothing at the recorded path: the media is away, not unfindable.
            (Some("absent"), _, _) => (Presence::Offline, note.clone(), Some("relink")),
            // Never probed: the project holds a reference and no knowledge of
            // the file. Where it was recorded decides which kind of nothing.
            (_, false, _) => match locator.why_not_local() {
                Some(why) => (Presence::NotOnThisMachine, Some(why), Some("relink")),
                None => (
                    Presence::NeverProbed,
                    Some(
                        "this media came from an imported cut and has never been read; \
                         nothing is known about what is in it"
                            .into(),
                    ),
                    Some("check_media"),
                ),
            },
            (_, true, Some(false)) => (
                Presence::Offline,
                note.clone()
                    .or_else(|| Some(format!("nothing is at {}", locator.display()))),
                Some("relink"),
            ),
            (_, true, _) => {
                // Probed and not known to be away. Whether this build can
                // decode it is a separate question, already answered.
                let undecodable = info.map(|i| !i.fully_decodable()).unwrap_or(false);
                if undecodable {
                    (
                        Presence::Undecodable,
                        Some("no decoder in this build handles every stream in it".into()),
                        None,
                    )
                } else {
                    (Presence::Present, None, None)
                }
            }
        };
        MediaState {
            presence,
            evidence,
            at,
            locator,
            detail,
            action,
        }
    }

    /// Why a picture cannot be read from this media, if it cannot.
    ///
    /// Every read of an original asks this, so no path can hand back a picture
    /// the project's own record says is not there — or say "not in this
    /// project" about a media that is.
    pub fn why_not_readable(&self) -> Option<String> {
        if self.presence.is_readable() {
            return None;
        }
        let what = self.presence.label();
        Some(match &self.detail {
            Some(d) => format!("{what}: {d}"),
            None => what.to_string(),
        })
    }

    pub fn to_json(&self) -> J {
        json!({
            "presence": self.presence.as_str(),
            "label": self.presence.label(),
            "readable": self.presence.is_readable(),
            "evidence": self.evidence.as_str(),
            "at": self.at,
            "recorded_at_location": self.locator.display(),
            "local_path": self.locator.as_path().map(|p| p.display().to_string()),
            "detail": self.detail,
            "action": self.action,
        })
    }
}

/// Seconds since the epoch, for recording when presence was established.
pub fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
