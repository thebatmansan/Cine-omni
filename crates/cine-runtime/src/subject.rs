//! What the user is working on, and everything the project connects to it.
//!
//! A filmmaker does not work in a department. They work on a *thing* — a
//! scene, a shot, a take — and need the departments' views of that thing. An
//! application built as department tabs makes the user hold the
//! correspondence in their own head, which is the failure of the tool-chain
//! this one exists to replace.
//!
//! So navigation has two axes. The **subject** is what is being worked on and
//! it persists; the **lens** is which craft's view of it is open and it
//! changes. Choosing Grade while looking at Scene 17 must leave you on Scene
//! 17.
//!
//! ## The relationships are the graph's, not this module's
//!
//! Nothing here knows that a scene has shots. It asks the project graph for a
//! node's neighbours — every edge, both directions — and groups them by the
//! *kind* of thing each neighbour is. The only judgement this module holds is
//! [`GROUPS`]: which craft a kind of thing belongs to, and which lens opens
//! it. That table is the vocabulary of a film, and it lives next to the
//! vocabulary of the graph rather than inside a React component, so that one
//! answer serves every surface and a new department is a row rather than a
//! rewrite.
//!
//! ## One hop, deliberately
//!
//! Neighbours are one edge away. A scene's takes are three hops off — scene,
//! shot, take — and reaching them by walking is what makes this navigation
//! rather than a tree dump of the project. Stepping is the point: each step
//! is a real edge somebody made, and the rail shows where that edge goes.
//!
//! ## Nothing is invented
//!
//! A group with no neighbours says so, and says it precisely: a subject that
//! *could* have storyboards and has none is a different fact from a build
//! that cannot do storyboards at all. Empty is never filled in.

use serde_json::{json, Value as J};
use ucg_core::{EntityId, Graph};
use ucg_schema::{attrs, kinds};

/// Which craft a kind of thing belongs to, and the lens that opens it.
///
/// Read in order; the first prefix that matches decides. A kind with no row
/// is still shown — under [`OTHER_GROUP`], with no lens — because a thing the
/// project holds and this table has not been taught about is a gap in the
/// table, not a reason to hide the thing.
pub const GROUPS: &[(&str, &str, Option<&str>)] = &[
    // (kind prefix, group, lens that opens it)
    ("script.", "Story", Some("script")),
    ("board.", "Visual", Some("storyboard")),
    ("previs.", "Visual", Some("previs")),
    ("logistics.shot", "Production", Some("production")),
    ("logistics.take", "Production", Some("production")),
    ("logistics.", "Production", Some("production")),
    ("asset.media", "Production", Some("media")),
    // A media's elementary streams: the thing itself, one layer down.
    ("media.derivative", "Production", Some("media")),
    ("media.stream", "Production", Some("media")),
    ("asset.", "Production", None),
    ("nle.", "Post", Some("editorial")),
    ("comp.", "Post", Some("vfx")),
    ("color.", "Post", Some("colour")),
    ("audio.", "Post", Some("audio")),
    ("sync.", "Post", Some("editorial")),
    ("project", "Project", None),
];

/// Where a kind this table has not been taught about is shown.
pub const OTHER_GROUP: &str = "Other";

/// The order groups are shown in: the order a film is made.
pub const GROUP_ORDER: &[&str] = &[
    "Project",
    "Story",
    "Visual",
    "Production",
    "Post",
    "Delivery",
    OTHER_GROUP,
];

/// The craft a kind belongs to, and the lens that opens it.
pub fn group_of(kind: &str) -> (&'static str, Option<&'static str>) {
    for (prefix, group, lens) in GROUPS {
        if kind.starts_with(prefix) {
            return (group, *lens);
        }
    }
    (OTHER_GROUP, None)
}

/// A kind in the words a person uses, singular.
pub fn kind_label(kind: &str) -> String {
    let tail = kind.split('.').next_back().unwrap_or(kind);
    let mut out = String::with_capacity(tail.len());
    for (i, part) in tail.split('_').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// How a node is named in the rail: its name, and the line under it.
fn headline(graph: &Graph, id: EntityId) -> (String, Option<String>) {
    let Some(n) = graph.node(id) else {
        return (String::new(), None);
    };
    let name = n.name();
    // A scene is known by its heading and number, not by an internal name.
    let sub = match n.kind.as_str() {
        kinds::SCRIPT_SCENE => n.get_str(attrs::HEADING).map(str::to_string),
        kinds::ASSET_MEDIA => n.get_str(attrs::MEDIA_PATH).map(str::to_string),
        _ => None,
    };
    let name = if name.is_empty() {
        kind_label(n.kind.as_str())
    } else {
        name
    };
    (name, sub)
}

fn entry(graph: &Graph, id: EntityId) -> Option<J> {
    let n = graph.node(id)?;
    let kind = n.kind.as_str().to_string();
    let (group, lens) = group_of(&kind);
    let (name, sub) = headline(graph, id);
    Some(json!({
        "id": id,
        "kind": kind,
        "kind_label": kind_label(&kind),
        "name": name,
        "sub": sub,
        "group": group,
        "lens": lens,
    }))
}

/// The containment path from the project down to this subject.
///
/// Where a thing was *derived* from another — a previs scene from a script
/// scene — that is followed too, because the thing it came from is where a
/// person would say it lives.
fn path(graph: &Graph, id: EntityId) -> Vec<J> {
    let mut out = Vec::new();
    let mut seen = vec![id];
    let mut cur = id;
    for _ in 0..32 {
        let up = graph
            .parent(cur)
            .or_else(|| {
                graph
                    .targets(cur, ucg_core::EdgeKind::DerivedFrom)
                    .into_iter()
                    .next()
            })
            .filter(|p| !seen.contains(p));
        let Some(up) = up else { break };
        if let Some(e) = entry(graph, up) {
            out.push(e);
        }
        seen.push(up);
        cur = up;
    }
    out.reverse();
    out
}

/// Everything one edge away, in both directions.
fn neighbours(graph: &Graph, id: EntityId) -> Vec<EntityId> {
    let mut out: Vec<EntityId> = Vec::new();
    for e in graph.out_edges(id) {
        if !out.contains(&e.to) {
            out.push(e.to);
        }
    }
    for e in graph.in_edges(id) {
        if !out.contains(&e.from) {
            out.push(e.from);
        }
    }
    out
}

/// What is being worked on, where it sits, and what the project connects to it.
///
/// `lens` is carried through untouched: the runtime does not decide which
/// craft's view is open, it only says which lens each related thing would
/// open. Losing the subject when the lens changes is the failure this exists
/// to prevent, and it is prevented by the subject never being derived from
/// the lens in the first place.
pub fn describe(graph: &Graph, id: EntityId) -> Result<J, String> {
    let Some(node) = graph.node(id) else {
        return Err("no such thing in this project".into());
    };
    let kind = node.kind.as_str().to_string();
    let (group, lens) = group_of(&kind);
    let (name, sub) = headline(graph, id);

    // Grouped by what each neighbour *is*, in the order a film is made.
    let mut grouped: Vec<(String, Vec<J>)> = Vec::new();
    for n in neighbours(graph, id) {
        let Some(e) = entry(graph, n) else { continue };
        let g = e["group"].as_str().unwrap_or(OTHER_GROUP).to_string();
        match grouped.iter_mut().find(|(name, _)| *name == g) {
            Some((_, list)) => list.push(e),
            None => grouped.push((g, vec![e])),
        }
    }
    grouped.sort_by_key(|(g, _)| {
        GROUP_ORDER
            .iter()
            .position(|o| o == g)
            .unwrap_or(GROUP_ORDER.len())
    });
    for (_, list) in grouped.iter_mut() {
        list.sort_by(|a, b| {
            (a["kind"].as_str(), a["name"].as_str()).cmp(&(b["kind"].as_str(), b["name"].as_str()))
        });
    }

    let related: Vec<J> = grouped
        .into_iter()
        .map(|(g, entries)| json!({ "group": g, "entries": entries }))
        .collect();

    Ok(json!({
        "subject": {
            "id": id,
            "kind": kind,
            "kind_label": kind_label(&kind),
            "name": name,
            "sub": sub,
            "group": group,
            "lens": lens,
        },
        "path": path(graph, id),
        "related": related,
    }))
}
