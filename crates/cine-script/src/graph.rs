//! Import/export between the [`Screenplay`] model and the Unified Context
//! Graph, plus the per-scene helpers reactors call.
//!
//! Graph shape produced by [`import_screenplay`]:
//!
//! ```text
//! project ─Contains─▶ script.document (id = from_key("script:{key}"))
//!    document ─Contains─▶ script.scene       (id = from_key("script:{key}:scene:{stable}"))
//!        scene ─Contains─▶ script.action | script.dialogue | ...   (id = from_key("{scene_key}:el:{i}"))
//!            dialogue ─Contains─▶ sync.alignment (id = derived(dialogue, "sync.alignment"))
//!    document ─Contains─▶ script.character | script.location | script.prop
//!        scene ─References─▶ character / location / prop
//! ```
//!
//! Character cues are folded into the dialogue-block nodes that follow them
//! (`character`, `extension`, `dual` attributes plus a private `script.cue`
//! ordinal), so the graph has no cue nodes; [`export_screenplay`] rebuilds
//! the cues. Every id is deterministic, so re-importing an edited screenplay
//! updates nodes in place and only produces ops for what changed.

use crate::error::{Error, Result};
use crate::model::{parse_scene_heading, Element, Screenplay};
use crate::stats::{dialogue_word_count, estimate_duration_seconds, page_eighths, word_list};
use crate::tag::auto_tag;
use std::collections::{HashMap, HashSet};
use ucg_core::{EdgeKind, EntityId, Graph, Tx, Value};
use ucg_schema::{attrs, kinds, Alignment, Component, SceneHeading, WordAlignment};

/// Node kind for Fountain boneyards (commented-out text). Module-private
/// because no other module consumes it.
pub const SCRIPT_BONEYARD: &str = "script.boneyard";
/// Document attribute holding the title page as a list of `[key, value]` pairs.
pub const TITLE_PAGE_ATTR: &str = "script.title_page";
/// Section level attribute (`# Act` = 1, `## Sequence` = 2).
pub const LEVEL_ATTR: &str = "script.level";
/// Ordinal of the character cue (within its scene) a dialogue-block node belongs to.
pub const CUE_ATTR: &str = "script.cue";
/// Scene attribute: the stable key the scene id was minted from.
pub const SCENE_KEY_ATTR: &str = "script.scene_key";

/// Id of the document node for a source key.
pub fn document_id(source_key: &str) -> EntityId {
    EntityId::from_key(&format!("script:{source_key}"))
}

/// Id of a scene node for a source key and stable scene key.
pub fn scene_id(source_key: &str, stable_key: &str) -> EntityId {
    EntityId::from_key(&format!("script:{source_key}:scene:{stable_key}"))
}

/// Id of the cast/location/prop entity node of `kind` named `name`.
pub fn entity_id(source_key: &str, kind: &str, name: &str) -> EntityId {
    let suffix = match kind {
        kinds::SCRIPT_CHARACTER => "character",
        kinds::SCRIPT_LOCATION => "location",
        _ => "prop",
    };
    EntityId::from_key(&format!("script:{source_key}:{suffix}:{name}"))
}

/// Graph node kind for an element (`None` for character cues, which are
/// folded into the following nodes).
pub fn element_kind(el: &Element) -> Option<&'static str> {
    Some(match el {
        Element::SceneHeading { .. } => kinds::SCRIPT_SCENE,
        Element::Action { .. } => kinds::SCRIPT_ACTION,
        Element::Character { .. } => return None,
        Element::Parenthetical { .. } => kinds::SCRIPT_PARENTHETICAL,
        Element::Dialogue { .. } => kinds::SCRIPT_DIALOGUE,
        Element::Transition { .. } => kinds::SCRIPT_TRANSITION,
        Element::Centered { .. } => kinds::SCRIPT_CENTERED,
        Element::Section { .. } => kinds::SCRIPT_SECTION,
        Element::Synopsis { .. } => kinds::SCRIPT_SYNOPSIS,
        Element::Note { .. } => kinds::SCRIPT_NOTE,
        Element::Lyric { .. } => kinds::SCRIPT_LYRIC,
        Element::PageBreak => kinds::SCRIPT_PAGE_BREAK,
        Element::Boneyard { .. } => SCRIPT_BONEYARD,
    })
}

/// Create `id` as `kind` if missing; if it exists with a different kind it is
/// deleted (with its subtree) and recreated.
fn ensure_kind(tx: &mut Tx, id: EntityId, kind: &str) -> Result<bool> {
    if let Some(n) = tx.graph().node(id) {
        if n.kind.as_str() == kind {
            return Ok(false);
        }
        tx.delete(id)?;
    }
    tx.create_with_id(id, kind)?;
    Ok(true)
}

fn require_kind(graph: &Graph, id: EntityId, kind: &str) -> Result<()> {
    let n = graph.require(id)?;
    if n.kind.as_str() != kind {
        return Err(Error::WrongKind {
            id,
            expected: kind.to_string(),
            actual: n.kind.to_string(),
        });
    }
    Ok(())
}

fn set_opt_str(tx: &mut Tx, id: EntityId, key: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(v) => tx.set(id, key, v)?,
        None => tx.remove_attr(id, key)?,
    }
    Ok(())
}

fn title_page_value(sp: &Screenplay) -> Value {
    Value::List(
        sp.title_page
            .iter()
            .map(|(k, v)| Value::List(vec![Value::Str(k.clone()), Value::Str(v.clone())]))
            .collect(),
    )
}

fn title_page_from_value(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_list)
        .map(|items| {
            items
                .iter()
                .filter_map(|pair| {
                    let l = pair.as_list()?;
                    Some((
                        l.first()?.as_str()?.to_string(),
                        l.get(1)?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The identity every scene of `sp` would have when imported under
/// `source_key`: `(stable key, scene key, entity id)`, in script order.
///
/// A scene's identity is its **scene number** when it has one, otherwise its
/// heading and how many scenes before it share that heading. So renaming a
/// numbered scene keeps it, and renaming an unnumbered one makes a different
/// scene — which is why numbering scenes is what locks a script for production.
/// The importer and anything that previews an import use this one function.
pub fn scene_identities(sp: &Screenplay, source_key: &str) -> Vec<(String, String, EntityId)> {
    let doc_key = format!("script:{source_key}");
    let mut occurrences: HashMap<&str, usize> = HashMap::new();
    let mut used_keys: HashSet<String> = HashSet::new();
    sp.scenes()
        .iter()
        .map(|scene| {
            let occ = occurrences.entry(scene.heading_text).or_insert(0);
            let mut stable = scene.stable_key(*occ);
            *occ += 1;
            while !used_keys.insert(stable.clone()) {
                stable.push_str("#dup");
            }
            let scene_key = format!("{doc_key}:scene:{stable}");
            let sid = EntityId::from_key(&scene_key);
            (stable, scene_key, sid)
        })
        .collect()
}

/// Import (or re-import) a screenplay under `project`. Returns the document id.
///
/// Ids are deterministic (see the module docs), so importing a modified
/// screenplay updates nodes in place, deletes scenes/elements that vanished,
/// and emits no ops for unchanged attributes.
pub fn import_screenplay(
    tx: &mut Tx,
    project: EntityId,
    sp: &Screenplay,
    source_key: &str,
) -> Result<EntityId> {
    let doc_key = format!("script:{source_key}");
    let doc = EntityId::from_key(&doc_key);
    ensure_kind(tx, doc, kinds::SCRIPT_DOCUMENT)?;
    if tx.graph().contains(project) {
        tx.link(project, doc, EdgeKind::Contains)?;
    }
    tx.set(doc, attrs::SOURCE_KEY, source_key)?;
    tx.set(
        doc,
        attrs::NAME,
        sp.title().filter(|t| !t.is_empty()).unwrap_or(source_key),
    )?;
    tx.set(doc, TITLE_PAGE_ATTR, title_page_value(sp))?;

    let scenes = sp.scenes();
    let tags = auto_tag(sp);
    let mut keep: HashSet<EntityId> = HashSet::new();

    // Cast, locations and props as entities under the document.
    let mut character_ids: HashMap<String, EntityId> = HashMap::new();
    for c in &tags.characters {
        let id = entity_id(source_key, kinds::SCRIPT_CHARACTER, &c.name);
        ensure_kind(tx, id, kinds::SCRIPT_CHARACTER)?;
        tx.link(doc, id, EdgeKind::Contains)?;
        tx.set(id, attrs::NAME, c.name.as_str())?;
        tx.set(id, "line_count", c.line_count)?;
        tx.set(id, "scene_count", c.scenes.len())?;
        keep.insert(id);
        character_ids.insert(c.name.clone(), id);
    }
    let mut location_ids: HashMap<String, EntityId> = HashMap::new();
    for l in &tags.locations {
        let id = entity_id(source_key, kinds::SCRIPT_LOCATION, &l.name);
        if location_ids.contains_key(&l.name) {
            continue;
        }
        ensure_kind(tx, id, kinds::SCRIPT_LOCATION)?;
        tx.link(doc, id, EdgeKind::Contains)?;
        tx.set(id, attrs::NAME, l.name.as_str())?;
        tx.set(id, attrs::INT_EXT, Value::from_serde(&l.int_ext)?)?;
        keep.insert(id);
        location_ids.insert(l.name.clone(), id);
    }
    let mut prop_ids: HashMap<String, EntityId> = HashMap::new();
    for p in &tags.props {
        let id = entity_id(source_key, kinds::SCRIPT_PROP, &p.name);
        ensure_kind(tx, id, kinds::SCRIPT_PROP)?;
        tx.link(doc, id, EdgeKind::Contains)?;
        tx.set(id, attrs::NAME, p.name.as_str())?;
        keep.insert(id);
        prop_ids.insert(p.name.clone(), id);
    }

    // Scenes.
    for (scene, (stable, scene_key, sid)) in scenes.iter().zip(scene_identities(sp, source_key)) {
        ensure_kind(tx, sid, kinds::SCRIPT_SCENE)?;
        tx.link(doc, sid, EdgeKind::Contains)?;
        keep.insert(sid);

        tx.set(sid, attrs::ORDER, scene.index)?;
        tx.set(sid, SCENE_KEY_ATTR, stable.as_str())?;
        tx.set(sid, attrs::HEADING, scene.heading_text)?;
        tx.set(
            sid,
            attrs::NAME,
            if scene.heading_text.is_empty() {
                "(preamble)"
            } else {
                scene.heading_text
            },
        )?;
        set_opt_str(tx, sid, attrs::SCENE_NUMBER, scene.number)?;
        match scene.heading {
            Some(h) => h.write(tx, sid)?,
            None => tx.remove_attr(sid, SceneHeading::KEY)?,
        }

        // Elements.
        let mut el_ids: HashSet<EntityId> = HashSet::new();
        let mut cue: Option<(usize, String, Option<String>, bool)> = None;
        let mut cue_count = 0usize;
        let mut idx = 0usize;
        for el in scene.body() {
            if let Element::Character {
                name,
                extension,
                dual,
            } = el
            {
                cue = Some((cue_count, name.clone(), extension.clone(), *dual));
                cue_count += 1;
                continue;
            }
            let Some(kind) = element_kind(el) else {
                continue;
            };
            let in_block = matches!(
                el,
                Element::Dialogue { .. } | Element::Parenthetical { .. } | Element::Lyric { .. }
            );
            if !in_block {
                cue = None;
            }
            let eid = EntityId::from_key(&format!("{scene_key}:el:{idx}"));
            ensure_kind(tx, eid, kind)?;
            tx.link(sid, eid, EdgeKind::Contains)?;
            el_ids.insert(eid);
            tx.set(eid, attrs::ORDER, idx)?;
            tx.set(eid, attrs::TEXT, el.text())?;
            match (in_block, &cue) {
                (true, Some((ordinal, name, ext, dual))) => {
                    tx.set(eid, CUE_ATTR, *ordinal)?;
                    tx.set(eid, attrs::CHARACTER, name.as_str())?;
                    set_opt_str(tx, eid, attrs::EXTENSION, ext.as_deref())?;
                    tx.set(eid, attrs::DUAL, *dual)?;
                }
                _ => {
                    for key in [CUE_ATTR, attrs::CHARACTER, attrs::EXTENSION, attrs::DUAL] {
                        tx.remove_attr(eid, key)?;
                    }
                }
            }
            match el {
                Element::Section { level, .. } => tx.set(eid, LEVEL_ATTR, i64::from(*level))?,
                _ => tx.remove_attr(eid, LEVEL_ATTR)?,
            }
            idx += 1;
        }
        for child in tx.graph().children(sid) {
            if !el_ids.contains(&child) {
                tx.delete(child)?;
            }
        }

        // Statistics and breakdown.
        let characters = scene.characters();
        let locations: Vec<String> = scene
            .heading
            .map(|h| h.location.trim().to_uppercase())
            .filter(|l| !l.is_empty())
            .into_iter()
            .collect();
        let props: Vec<String> = tags
            .props
            .iter()
            .filter(|p| p.scenes.contains(&scene.index))
            .map(|p| p.name.clone())
            .collect();
        tx.set(sid, attrs::CHARACTERS, characters.clone())?;
        tx.set(sid, attrs::LOCATIONS, locations.clone())?;
        tx.set(sid, attrs::PROPS, props.clone())?;
        tx.set(sid, attrs::PAGE_EIGHTHS, i64::from(page_eighths(scene)))?;
        tx.set(sid, attrs::EST_DURATION_S, estimate_duration_seconds(scene))?;
        tx.set(sid, attrs::WORDS, dialogue_word_count(scene))?;

        // References to cast/location/prop entities.
        let mut desired: Vec<EntityId> = Vec::new();
        desired.extend(characters.iter().filter_map(|c| character_ids.get(c)));
        desired.extend(locations.iter().filter_map(|l| location_ids.get(l)));
        desired.extend(props.iter().filter_map(|p| prop_ids.get(p)));
        let desired_set: HashSet<EntityId> = desired.iter().copied().collect();
        for existing in tx.graph().targets(sid, EdgeKind::References) {
            if !desired_set.contains(&existing) {
                tx.unlink(sid, existing, EdgeKind::References)?;
            }
        }
        for target in desired {
            tx.link(sid, target, EdgeKind::References)?;
        }
    }

    // Remove scenes and entities that vanished.
    for child in tx.graph().children(doc) {
        if !keep.contains(&child) {
            tx.delete(child)?;
        }
    }
    Ok(doc)
}

/// Rebuild the element list of one scene node (heading first, when present).
pub fn scene_elements(graph: &Graph, scene: EntityId) -> Vec<Element> {
    let mut out = Vec::new();
    let Some(node) = graph.node(scene) else {
        return out;
    };
    let heading_text = node.get_str(attrs::HEADING).unwrap_or("");
    if !heading_text.is_empty() {
        let heading = SceneHeading::read(node).unwrap_or_else(|| parse_scene_heading(heading_text));
        out.push(Element::SceneHeading {
            text: heading_text.to_string(),
            scene_number: node.get_str(attrs::SCENE_NUMBER).map(str::to_string),
            heading,
        });
    }
    let mut current_cue: Option<i64> = None;
    for child in graph.children(scene) {
        let Some(n) = graph.node(child) else { continue };
        let text = n.get_str(attrs::TEXT).unwrap_or("").to_string();
        let kind = n.kind.as_str();
        let in_block = matches!(
            kind,
            kinds::SCRIPT_DIALOGUE | kinds::SCRIPT_PARENTHETICAL | kinds::SCRIPT_LYRIC
        );
        let cue = if in_block { n.get_int(CUE_ATTR) } else { None };
        match (cue, current_cue) {
            (Some(c), Some(cur)) if c == cur => {}
            (Some(_), _) => {
                out.push(Element::Character {
                    name: n.get_str(attrs::CHARACTER).unwrap_or("").to_string(),
                    extension: n.get_str(attrs::EXTENSION).map(str::to_string),
                    dual: n.get_bool(attrs::DUAL).unwrap_or(false),
                });
            }
            (None, _) => {}
        }
        current_cue = cue;
        let el = match kind {
            kinds::SCRIPT_ACTION => Element::Action { text },
            kinds::SCRIPT_DIALOGUE => Element::Dialogue { text },
            kinds::SCRIPT_PARENTHETICAL => Element::Parenthetical { text },
            kinds::SCRIPT_TRANSITION => Element::Transition { text },
            kinds::SCRIPT_CENTERED => Element::Centered { text },
            kinds::SCRIPT_SECTION => {
                let level = n.get_int(LEVEL_ATTR).unwrap_or(1).clamp(1, 255) as u8;
                Element::Section { level, text }
            }
            kinds::SCRIPT_SYNOPSIS => Element::Synopsis { text },
            kinds::SCRIPT_NOTE => Element::Note { text },
            kinds::SCRIPT_LYRIC => Element::Lyric { text },
            kinds::SCRIPT_PAGE_BREAK => Element::PageBreak,
            SCRIPT_BONEYARD => Element::Boneyard { text },
            _ => continue,
        };
        out.push(el);
    }
    out
}

/// Rebuild a [`Screenplay`] from a document node (the inverse of
/// [`import_screenplay`]).
pub fn export_screenplay(graph: &Graph, document: EntityId) -> Result<Screenplay> {
    require_kind(graph, document, kinds::SCRIPT_DOCUMENT)?;
    let mut sp = Screenplay::new();
    sp.title_page = title_page_from_value(graph.attr(document, TITLE_PAGE_ATTR));
    for child in graph.children(document) {
        if graph
            .node(child)
            .is_some_and(|n| n.kind.as_str() == kinds::SCRIPT_SCENE)
        {
            sp.elements.extend(scene_elements(graph, child));
        }
    }
    Ok(sp)
}

/// Recompute `characters`, `words`, `page_eighths` and `est_duration_s` of a
/// scene from its current child elements (used after a single line edit).
pub fn recompute_scene_stats(tx: &mut Tx, scene: EntityId) -> Result<()> {
    require_kind(tx.graph(), scene, kinds::SCRIPT_SCENE)?;
    let sp = Screenplay {
        title_page: vec![],
        elements: scene_elements(tx.graph(), scene),
    };
    let views = sp.scenes();
    let (characters, words, eighths, duration) = match views.first() {
        Some(v) => (
            v.characters(),
            dialogue_word_count(v),
            page_eighths(v),
            estimate_duration_seconds(v),
        ),
        None => (Vec::new(), 0, 1, 0.0),
    };
    tx.set(scene, attrs::CHARACTERS, characters)?;
    tx.set(scene, attrs::WORDS, words)?;
    tx.set(scene, attrs::PAGE_EIGHTHS, i64::from(eighths))?;
    tx.set(scene, attrs::EST_DURATION_S, duration)?;
    Ok(())
}

/// `(dialogue node, words)` for every dialogue line of a scene, in order.
pub fn scene_dialogue_words(graph: &Graph, scene: EntityId) -> Vec<(EntityId, Vec<String>)> {
    graph
        .children(scene)
        .into_iter()
        .filter_map(|id| {
            let n = graph.node(id)?;
            if n.kind.as_str() != kinds::SCRIPT_DIALOGUE {
                return None;
            }
            Some((id, word_list(n.get_str(attrs::TEXT).unwrap_or(""))))
        })
        .collect()
}

/// Id of the alignment node attached to a dialogue line.
pub fn alignment_id(dialogue: EntityId) -> EntityId {
    EntityId::derived(dialogue, kinds::SYNC_ALIGNMENT)
}

/// Write a word alignment for a dialogue line. The alignment node is
/// contained in the dialogue and, when `target` is given, `SyncedTo` it.
pub fn apply_alignment(
    tx: &mut Tx,
    dialogue: EntityId,
    target: Option<EntityId>,
    words: Vec<WordAlignment>,
) -> Result<EntityId> {
    tx.graph().require(dialogue)?;
    let id = alignment_id(dialogue);
    tx.ensure(id, kinds::SYNC_ALIGNMENT)?;
    tx.link(dialogue, id, EdgeKind::Contains)?;
    tx.set(id, attrs::NAME, "alignment")?;
    for existing in tx.graph().targets(id, EdgeKind::SyncedTo) {
        if Some(existing) != target {
            tx.unlink(id, existing, EdgeKind::SyncedTo)?;
        }
    }
    if let Some(t) = target {
        tx.link(id, t, EdgeKind::SyncedTo)?;
    }
    Alignment {
        target,
        words,
        stale: false,
    }
    .write(tx, id)?;
    tx.set(id, attrs::STALE, false)?;
    Ok(id)
}

/// Flag every alignment under `scene` as stale. Returns the number of
/// alignment nodes found.
pub fn mark_alignments_stale(tx: &mut Tx, scene: EntityId) -> Result<usize> {
    tx.graph().require(scene)?;
    let targets: Vec<EntityId> = tx
        .graph()
        .descendants(scene)
        .into_iter()
        .filter(|id| {
            tx.graph()
                .node(*id)
                .is_some_and(|n| n.kind.as_str() == kinds::SYNC_ALIGNMENT)
        })
        .collect();
    for id in &targets {
        let mut a = Alignment::read_from(tx.graph(), *id).unwrap_or_default();
        if !a.stale {
            a.stale = true;
            a.write(tx, *id)?;
        }
        tx.set(*id, attrs::STALE, true)?;
    }
    Ok(targets.len())
}

/// All alignment nodes under a scene (or any subtree root).
pub fn alignments_under(graph: &Graph, root: EntityId) -> Vec<EntityId> {
    graph
        .descendants(root)
        .into_iter()
        .filter(|id| {
            graph
                .node(*id)
                .is_some_and(|n| n.kind.as_str() == kinds::SYNC_ALIGNMENT)
        })
        .collect()
}
