//! A caption linked to the line of dialogue it transcribes.
//!
//! The relationship runs through the project, not through text:
//!
//! ```text
//! script.dialogue ── in ──▶ script.scene ◀── SyncedTo ── nle.clip ── under ── nle.caption
//! ```
//!
//! A caption sits over clips in the cut; those clips are bound to scenes; the
//! lines it could transcribe are the dialogue of those scenes. Linking one of
//! them records a `DerivedFrom` edge and, because the edge alone is not enough,
//! what the line *was*: its scene, its speaker, which of that speaker's lines
//! in the scene it is, and its text as transcribed.
//!
//! ## Why the edge is not enough
//!
//! Script elements are identified by their position in a scene. Inserting an
//! action line above a speech gives that speech a new id and puts a different
//! element at the old one. An edge kept blindly would then describe the wrong
//! line — the quiet failure this exists to prevent. So the link is resolved
//! each time the script or the cut changes, within its own scene and speaker:
//!
//! 1. the same speaker still says exactly that text → it is the same line, and
//!    if it moved the edge follows it, with no flag;
//! 2. otherwise the line the edge points at, or failing that the speaker's most
//!    similar line in the scene, is close enough to be a rewrite of it → the
//!    caption is flagged, with the old words and the new;
//! 3. otherwise the line is gone → flagged as cut.
//!
//! "Close enough" is shared words over all words, a similarity that is only
//! ever compared within one speaker in one scene. It decides between *changed*
//! and *cut*; it never decides whether there is a problem, and both answers
//! flag the caption.
//!
//! The caption is also flagged when it no longer sits over any clip of the
//! scene its line belongs to: a re-cut can move a caption's picture out from
//! under it without anybody touching the caption.
//!
//! Accepting the line takes its current words as the transcription and clears
//! the flag; undo reverses that like any other commit. Reverting a rewrite
//! clears it by itself, because the line is then exactly what was transcribed.

use crate::error::{Error, Result};
use crate::graph::{export_timeline, scenes_of_clip};
use crate::otio::{Item, TrackKind};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use ucg_core::{EdgeKind, EntityId, Graph, TimeRange, Tx};
use ucg_schema::{attrs, kinds};

/// The scene of the linked line.
pub const LINE_SCENE: &str = "nle.caption_line_scene";
/// Its speaker.
pub const LINE_CHARACTER: &str = "nle.caption_line_character";
/// Which of that speaker's lines in the scene it is, from 0.
pub const LINE_ORDINAL: &str = "nle.caption_line_ordinal";
/// Its words as transcribed.
pub const LINE_TEXT: &str = "nle.caption_line_text";
/// Why the caption needs looking at, when it does.
pub const REVIEW: &str = "nle.caption_review";

/// One line of dialogue as a caption sees it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Line {
    pub id: EntityId,
    pub scene: EntityId,
    pub character: String,
    /// Which of this speaker's lines in the scene, from 0.
    pub ordinal: usize,
    pub text: String,
}

/// The dialogue of a scene, in script order.
pub fn dialogue_lines(graph: &Graph, scene: EntityId) -> Vec<Line> {
    let mut rows: Vec<(i64, EntityId)> = graph
        .children(scene)
        .into_iter()
        .filter_map(|id| {
            let n = graph.node(id)?;
            (n.kind.as_str() == kinds::SCRIPT_DIALOGUE)
                .then(|| (n.get_int(attrs::ORDER).unwrap_or(0), id))
        })
        .collect();
    rows.sort();
    let mut count: HashMap<String, usize> = HashMap::new();
    rows.into_iter()
        .filter_map(|(_, id)| {
            let n = graph.node(id)?;
            let character = n.get_str(attrs::CHARACTER).unwrap_or("").to_string();
            let ordinal = {
                let c = count.entry(character.clone()).or_default();
                *c += 1;
                *c - 1
            };
            Some(Line {
                id,
                scene,
                character,
                ordinal,
                text: n.get_str(attrs::TEXT).unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Shared words over all words, on lower-cased letters and digits.
fn similarity(a: &str, b: &str) -> f64 {
    let words = |s: &str| -> BTreeSet<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let (a, b) = (words(a), words(b));
    let union = a.union(&b).count();
    if union == 0 {
        return 1.0;
    }
    a.intersection(&b).count() as f64 / union as f64
}

/// Text as a caption and a line compare: whitespace collapsed, case ignored.
fn normalise(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn scene_heading(graph: &Graph, scene: EntityId) -> String {
    graph
        .node(scene)
        .and_then(|n| n.get_str(attrs::HEADING))
        .unwrap_or("an untitled scene")
        .to_string()
}

// ---------------------------------------------------------------------------
// where a caption is
// ---------------------------------------------------------------------------

/// The sequence a caption belongs to.
pub fn timeline_of_caption(graph: &Graph, caption: EntityId) -> Option<EntityId> {
    let track = graph.parent(caption)?;
    graph
        .targets(track, EdgeKind::References)
        .into_iter()
        .find(|t| {
            graph
                .node(*t)
                .is_some_and(|n| n.kind.as_str() == kinds::NLE_TIMELINE)
        })
}

/// Enabled picture clips of a sequence with where they sit, for asking what is
/// under a caption. Built once per sequence and reused across its captions.
pub struct CutIndex {
    clips: Vec<(TimeRange, EntityId)>,
}

impl CutIndex {
    pub fn of(graph: &Graph, timeline: EntityId) -> Result<CutIndex> {
        let composed = export_timeline(graph, timeline)?;
        let mut clips = Vec::new();
        for track in composed.tracks() {
            if track.kind != TrackKind::Video || !track.enabled {
                continue;
            }
            for (i, item) in track.children.iter().enumerate() {
                let Item::Clip(c) = item else { continue };
                if !c.enabled {
                    continue;
                }
                if let (Some(range), Some(id)) = (track.range_of_child(i), item.ucg_id()) {
                    clips.push((range, id));
                }
            }
        }
        Ok(CutIndex { clips })
    }

    /// Scenes bound to a clip that overlaps `range`, in the order they first
    /// appear under it.
    pub fn scenes_under(&self, graph: &Graph, range: TimeRange) -> Vec<EntityId> {
        let mut under: Vec<(TimeRange, EntityId)> = self
            .clips
            .iter()
            .filter(|(r, _)| r.overlaps(&range))
            .copied()
            .collect();
        under.sort_by_key(|(r, _)| r.start_time);
        let mut out = Vec::new();
        for (_, clip) in under {
            for scene in scenes_of_clip(graph, clip) {
                let is_scene = graph
                    .node(scene)
                    .is_some_and(|n| n.kind.as_str() == kinds::SCRIPT_SCENE);
                if is_scene && !out.contains(&scene) {
                    out.push(scene);
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// candidates and linking
// ---------------------------------------------------------------------------

/// A line a caption could transcribe.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Candidate {
    pub line: Line,
    pub heading: String,
    /// How alike the caption's text and the line are, from 0 to 1. For
    /// ordering only: every candidate is a line of a scene under the caption.
    pub score: f64,
}

/// The lines a caption could transcribe: the dialogue of every scene bound to
/// a clip beneath it, most alike first.
pub fn candidates(graph: &Graph, index: &CutIndex, range: TimeRange, text: &str) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = index
        .scenes_under(graph, range)
        .into_iter()
        .flat_map(|scene| {
            let heading = scene_heading(graph, scene);
            dialogue_lines(graph, scene)
                .into_iter()
                .map(move |line| (line, heading.clone()))
        })
        .map(|(line, heading)| Candidate {
            score: similarity(text, &line.text),
            line,
            heading,
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

/// Link a caption to a line of dialogue. The line's current words become the
/// transcription it is checked against, and any flag is cleared.
pub fn link(tx: &mut Tx, caption: EntityId, line: EntityId) -> Result<()> {
    let g = tx.graph();
    let cn = g.require(caption)?;
    if cn.kind.as_str() != kinds::NLE_CAPTION {
        return Err(Error::Edit("only a caption can be linked to a line".into()));
    }
    let ln = g.require(line)?;
    if ln.kind.as_str() != kinds::SCRIPT_DIALOGUE {
        return Err(Error::Edit(format!(
            "a caption transcribes dialogue, and this is `{}`",
            ln.kind.as_str()
        )));
    }
    let scene = g
        .ancestor_of_kind(line, kinds::SCRIPT_SCENE)
        .ok_or_else(|| Error::Edit("the line is in no scene".into()))?;
    let resolved = dialogue_lines(g, scene)
        .into_iter()
        .find(|l| l.id == line)
        .ok_or_else(|| Error::Edit("the line is not dialogue of its scene".into()))?;
    for old in g.targets(caption, EdgeKind::DerivedFrom) {
        if old != line {
            tx.unlink(caption, old, EdgeKind::DerivedFrom)?;
        }
    }
    tx.link(caption, line, EdgeKind::DerivedFrom)?;
    tx.set(caption, LINE_SCENE, scene.to_string())?;
    tx.set(caption, LINE_CHARACTER, resolved.character.as_str())?;
    tx.set(caption, LINE_ORDINAL, resolved.ordinal as i64)?;
    tx.set(caption, LINE_TEXT, resolved.text.as_str())?;
    tx.remove_attr(caption, REVIEW)?;
    Ok(())
}

/// Remove a caption's link.
pub fn unlink(tx: &mut Tx, caption: EntityId) -> Result<()> {
    for old in tx.graph().targets(caption, EdgeKind::DerivedFrom) {
        tx.unlink(caption, old, EdgeKind::DerivedFrom)?;
    }
    for key in [LINE_SCENE, LINE_CHARACTER, LINE_ORDINAL, LINE_TEXT, REVIEW] {
        tx.remove_attr(caption, key)?;
    }
    Ok(())
}

/// Link, in one commit, every caption of a track whose text is exactly one of
/// the lines of the scenes beneath it — and only when exactly one line
/// matches, so nothing ambiguous is decided for anybody. Returns how many
/// were linked.
pub fn link_exact(tx: &mut Tx, timeline: EntityId, language: &str) -> Result<usize> {
    let track = crate::graph::export_captions(tx.graph(), timeline, language)?
        .ok_or_else(|| Error::Edit(format!("the sequence has no `{language}` captions")))?;
    let index = CutIndex::of(tx.graph(), timeline)?;
    let mut plans = Vec::new();
    for c in &track.captions {
        let Some(key) = &c.key else { continue };
        let id = crate::graph::caption_node(timeline, language, key);
        if tx.graph().attr(id, LINE_SCENE).is_some() {
            continue;
        }
        let text = normalise(&c.lines.join(" "));
        let exact: Vec<Candidate> = candidates(tx.graph(), &index, c.record, &text)
            .into_iter()
            .filter(|k| normalise(&k.line.text) == text)
            .collect();
        if exact.len() == 1 {
            plans.push((id, exact[0].line.id));
        }
    }
    for (caption, line) in &plans {
        link(tx, *caption, *line)?;
    }
    Ok(plans.len())
}

// ---------------------------------------------------------------------------
// resolution
// ---------------------------------------------------------------------------

/// What a link resolves to now.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// The same speaker still says exactly the transcribed words.
    Same(Line),
    /// A line close enough to be a rewrite of the transcribed one.
    Changed { line: Line, was: String },
    /// The line is no longer in its scene.
    Cut { was: String },
    /// The scene itself is gone.
    SceneGone { was: String },
}

/// The recorded half of a link.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    pub scene: EntityId,
    pub character: String,
    pub ordinal: usize,
    pub text: String,
}

pub fn recorded(graph: &Graph, caption: EntityId) -> Option<Recorded> {
    let n = graph.node(caption)?;
    Some(Recorded {
        scene: n.get_str(LINE_SCENE).and_then(EntityId::parse)?,
        character: n.get_str(LINE_CHARACTER).unwrap_or("").to_string(),
        ordinal: n.get_int(LINE_ORDINAL).unwrap_or(0).max(0) as usize,
        text: n.get_str(LINE_TEXT).unwrap_or("").to_string(),
    })
}

/// Resolve a link against the script as it is now.
pub fn resolve(graph: &Graph, caption: EntityId, link: &Recorded) -> Resolution {
    if !graph
        .node(link.scene)
        .is_some_and(|n| n.kind.as_str() == kinds::SCRIPT_SCENE)
    {
        return Resolution::SceneGone {
            was: link.text.clone(),
        };
    }
    let own: Vec<Line> = dialogue_lines(graph, link.scene)
        .into_iter()
        .filter(|l| l.character == link.character)
        .collect();

    // 1. Exactly those words from that speaker: the same line, wherever it is.
    let want = normalise(&link.text);
    let mut exact: Vec<&Line> = own.iter().filter(|l| normalise(&l.text) == want).collect();
    exact.sort_by_key(|l| l.ordinal.abs_diff(link.ordinal));
    if let Some(line) = exact.first() {
        return Resolution::Same((*line).clone());
    }

    // 2. The line the edge points at, if it is still that speaker's and alike.
    const REWRITE: f64 = 0.3;
    let edge = graph.targets(caption, EdgeKind::DerivedFrom);
    if let Some(line) = own
        .iter()
        .find(|l| edge.contains(&l.id) && similarity(&l.text, &link.text) >= REWRITE)
    {
        return Resolution::Changed {
            line: line.clone(),
            was: link.text.clone(),
        };
    }
    // ... or the speaker's most alike line in the scene.
    if let Some(line) = own
        .iter()
        .map(|l| (similarity(&l.text, &link.text), l))
        .filter(|(s, _)| *s >= REWRITE)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, l)| l)
    {
        return Resolution::Changed {
            line: line.clone(),
            was: link.text.clone(),
        };
    }

    // 3. Gone.
    Resolution::Cut {
        was: link.text.clone(),
    }
}

/// Where a linked caption should stand: which line its edge points at, and
/// why it needs review, if it does.
#[derive(Clone, Debug, PartialEq)]
pub struct Standing {
    pub line: Option<Line>,
    pub review: Option<String>,
}

/// Work out a linked caption's standing against the script and the cut.
pub fn standing(graph: &Graph, caption: EntityId, link: &Recorded, index: &CutIndex) -> Standing {
    let heading = scene_heading(graph, link.scene);
    let who = if link.character.is_empty() {
        "The".to_string()
    } else {
        format!("{}'s", link.character)
    };
    let mut reasons = Vec::new();
    let line = match resolve(graph, caption, link) {
        Resolution::Same(line) => Some(line),
        Resolution::Changed { line, was } => {
            reasons.push(format!(
                "{who} line in {heading} was rewritten: \"{was}\" is now \"{}\"",
                line.text
            ));
            Some(line)
        }
        Resolution::Cut { was } => {
            reasons.push(format!("{who} line \"{was}\" is no longer in {heading}"));
            None
        }
        Resolution::SceneGone { was } => {
            reasons.push(format!(
                "the scene of {} line \"{was}\" is no longer in the script",
                who.to_lowercase()
            ));
            None
        }
    };
    if graph.contains(link.scene) {
        if let Some(range) = graph
            .node(caption)
            .and_then(|n| n.get(attrs::RANGE))
            .and_then(ucg_core::Value::as_range)
        {
            if !index.scenes_under(graph, range).contains(&link.scene) {
                reasons.push(format!(
                    "the caption no longer sits over {heading} in the cut"
                ));
            }
        }
    }
    Standing {
        line,
        review: (!reasons.is_empty()).then(|| reasons.join("; ")),
    }
}

/// Every linked caption, grouped by the sequence it belongs to.
pub fn linked_captions(graph: &Graph) -> BTreeMap<EntityId, Vec<EntityId>> {
    let mut out: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
    for n in graph.of_kind(kinds::NLE_CAPTION) {
        if n.get(LINE_SCENE).is_none() {
            continue;
        }
        if let Some(t) = timeline_of_caption(graph, n.id) {
            out.entry(t).or_default().push(n.id);
        }
    }
    out
}

/// Bring a caption's edge and flag in line with its standing. Writes nothing
/// when they already agree, so a reactor running this converges.
pub fn apply_standing(tx: &mut Tx, caption: EntityId, standing: &Standing) -> Result<()> {
    let current = tx.graph().targets(caption, EdgeKind::DerivedFrom);
    match &standing.line {
        Some(line) => {
            for old in &current {
                if *old != line.id {
                    tx.unlink(caption, *old, EdgeKind::DerivedFrom)?;
                }
            }
            tx.link(caption, line.id, EdgeKind::DerivedFrom)?;
            // A line that only moved keeps its transcription; its position is
            // updated so the next resolution starts from where it is.
            if standing.review.is_none() {
                tx.set(caption, LINE_ORDINAL, line.ordinal as i64)?;
            }
        }
        None => {
            for old in &current {
                tx.unlink(caption, *old, EdgeKind::DerivedFrom)?;
            }
        }
    }
    let now = tx
        .graph()
        .node(caption)
        .and_then(|n| n.get_str(REVIEW))
        .map(str::to_string);
    match (&standing.review, now) {
        (Some(want), Some(have)) if *want == have => {}
        (Some(want), _) => tx.set(caption, REVIEW, want.as_str())?,
        (None, Some(_)) => tx.remove_attr(caption, REVIEW)?,
        (None, None) => {}
    }
    Ok(())
}

/// Every linked caption's standing, computing each sequence's cut once.
pub fn review_all(graph: &Graph) -> Vec<(EntityId, Standing)> {
    let mut out = Vec::new();
    for (timeline, captions) in linked_captions(graph) {
        let Ok(index) = CutIndex::of(graph, timeline) else {
            continue;
        };
        for caption in captions {
            if let Some(link) = recorded(graph, caption) {
                out.push((caption, standing(graph, caption, &link, &index)));
            }
        }
    }
    out
}

/// Whether a caption's stored edge and flag already match a standing.
pub fn agrees(graph: &Graph, caption: EntityId, standing: &Standing) -> bool {
    let edges = graph.targets(caption, EdgeKind::DerivedFrom);
    let edge_ok = match &standing.line {
        Some(line) => edges == vec![line.id],
        None => edges.is_empty(),
    };
    let review = graph
        .node(caption)
        .and_then(|n| n.get_str(REVIEW))
        .map(str::to_string);
    let ordinal_ok = match (&standing.line, &standing.review) {
        (Some(line), None) => {
            graph.node(caption).and_then(|n| n.get_int(LINE_ORDINAL)) == Some(line.ordinal as i64)
        }
        _ => true,
    };
    edge_ok && ordinal_ok && review == standing.review
}

/// Accept the line as it is now: its words become the transcription the
/// caption is checked against, and the flag clears. Refused when the line is
/// gone, because there is nothing to accept — unlink the caption instead.
pub fn accept(tx: &mut Tx, caption: EntityId) -> Result<()> {
    let link = recorded(tx.graph(), caption)
        .ok_or_else(|| Error::Edit("the caption is not linked to a line".into()))?;
    match resolve(tx.graph(), caption, &link) {
        Resolution::Same(line) | Resolution::Changed { line, .. } => {
            link_line(tx, caption, &line)
        }
        Resolution::Cut { was } | Resolution::SceneGone { was } => Err(Error::Edit(format!(
            "\"{was}\" is no longer in the script, so there is no line to accept; unlink the caption"
        ))),
    }
}

fn link_line(tx: &mut Tx, caption: EntityId, line: &Line) -> Result<()> {
    link(tx, caption, line.id)
}

/// Captions that transcribe a line.
pub fn captions_of_line(graph: &Graph, line: EntityId) -> Vec<EntityId> {
    graph
        .sources(line, EdgeKind::DerivedFrom)
        .into_iter()
        .filter(|c| {
            graph
                .node(*c)
                .is_some_and(|n| n.kind.as_str() == kinds::NLE_CAPTION)
        })
        .collect()
}
