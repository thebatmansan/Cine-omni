//! Mapping between [`CompGraph`] and the Unified Context Graph.
//!
//! A comp graph is a `comp.graph` entity derived from its shot
//! (`EntityId::derived(shot, kinds::COMP_GRAPH)`) with a `DependsOn` edge to
//! the shot. Its nodes are `comp.node` children (ids derived from the node
//! name) carrying `kind` / `params` / `position` / `bypass` attributes; the
//! links live in a `links` attribute on the graph entity. Every writer is
//! idempotent: re-running it on an unchanged graph produces zero ops.

use crate::error::{Error, Result};
use crate::graph_model::{
    CompGraph, CompNode, Link, MergeOp, NodeId, NodeKind, Param, PassKind, ProceduralKind,
    ReadSource,
};
use indexmap::IndexMap;
use std::collections::HashSet;
use ucg_core::{EdgeKind, EntityId, Graph, TimeRange, Tx, Value};
use ucg_schema::{attrs, kinds};

/// Attribute holding a node's kind name.
pub const ATTR_KIND: &str = "kind";
/// Attribute holding a node's parameter map.
pub const ATTR_PARAMS: &str = "params";
/// Attribute holding a node's editor position.
pub const ATTR_POSITION: &str = "position";
/// Attribute holding a node's bypass flag.
pub const ATTR_BYPASS: &str = "bypass";
/// Attribute holding a node's in-graph id.
pub const ATTR_NODE_ID: &str = "node_id";
/// Attribute on the graph entity holding the links.
pub const ATTR_LINKS: &str = "links";

/// Entity id of the comp graph of a shot.
pub fn comp_id_for_shot(shot: EntityId) -> EntityId {
    EntityId::derived(shot, kinds::COMP_GRAPH)
}

/// Entity id of a comp node by name.
pub fn node_entity_id(comp: EntityId, name: &str) -> EntityId {
    EntityId::derived(comp, &format!("node:{name}"))
}

/// The default graph a new shot comp starts with:
/// `Read(beauty) → ZDefocus ← Read(depth)`, merged over a procedural
/// backdrop, graded and written.
pub fn template_graph(shot_name: &str) -> CompGraph {
    let mut g = CompGraph::new();
    let beauty = g
        .add(
            CompNode::read_previs("beauty", EntityId::nil(), PassKind::Beauty, EntityId::nil())
                .at(0.0, 0.0),
        )
        .expect("template names are unique");
    let depth = g
        .add(
            CompNode::read_previs("depth", EntityId::nil(), PassKind::Depth, EntityId::nil())
                .at(0.0, 150.0),
        )
        .expect("template names are unique");
    let defocus = g
        .add(CompNode::new("defocus", NodeKind::ZDefocus).at(250.0, 0.0))
        .expect("unique");
    let backdrop = g
        .add(
            CompNode::procedural("backdrop", ProceduralKind::Gradient, 1920, 1080)
                .with_param("color1", [0.02f32, 0.02, 0.03, 1.0])
                .with_param("color2", [0.12f32, 0.12, 0.15, 1.0])
                .with_param("angle", 90.0f64)
                .at(250.0, 300.0),
        )
        .expect("unique");
    let merge = g
        .add(CompNode::merge("merge", MergeOp::Over).at(500.0, 0.0))
        .expect("unique");
    let grade = g
        .add(CompNode::new("grade", NodeKind::Grade).at(750.0, 0.0))
        .expect("unique");
    let safe: String = shot_name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let write = g
        .add(CompNode::write("write", &format!("comp/{safe}.####.exr")).at(1000.0, 0.0))
        .expect("unique");
    g.connect(beauty, 0, defocus, 0).expect("valid template");
    g.connect(depth, 0, defocus, 1).expect("valid template");
    g.connect(defocus, 0, merge, 0).expect("valid template");
    g.connect(backdrop, 0, merge, 1).expect("valid template");
    g.connect(merge, 0, grade, 0).expect("valid template");
    g.connect(grade, 0, write, 0).expect("valid template");
    g
}

fn comp_node_children(graph: &Graph, comp: EntityId) -> Vec<EntityId> {
    graph
        .children(comp)
        .into_iter()
        .filter(|c| {
            graph
                .node(*c)
                .map(|n| n.kind.as_str() == kinds::COMP_NODE)
                .unwrap_or(false)
        })
        .collect()
}

fn require_comp(graph: &Graph, comp: EntityId) -> Result<()> {
    let node = graph.node(comp).ok_or(ucg_core::UcgError::NotFound(comp))?;
    if node.kind.as_str() != kinds::COMP_GRAPH {
        return Err(Error::InvalidEntity(format!(
            "{comp} is a {} not a {}",
            node.kind,
            kinds::COMP_GRAPH
        )));
    }
    Ok(())
}

/// Ensure the shot has a comp graph entity (creating it with the template
/// graph when missing). Idempotent: a second call produces no ops.
pub fn ensure_comp_for_shot(tx: &mut Tx, shot: EntityId) -> Result<EntityId> {
    let shot_name = tx
        .graph()
        .node(shot)
        .ok_or(ucg_core::UcgError::NotFound(shot))?
        .name();
    let comp = comp_id_for_shot(shot);
    tx.ensure(comp, kinds::COMP_GRAPH)?;
    tx.set(comp, attrs::NAME, format!("Comp {shot_name}"))?;
    tx.link(comp, shot, EdgeKind::DependsOn)?;
    if comp_node_children(tx.graph(), comp).is_empty() {
        store_graph(tx, comp, &template_graph(&shot_name))?;
    }
    Ok(comp)
}

/// Write a comp graph into the UCG under `comp`, removing nodes that
/// vanished. Idempotent.
pub fn store_graph(tx: &mut Tx, comp: EntityId, graph: &CompGraph) -> Result<()> {
    require_comp(tx.graph(), comp)?;
    let mut keep = HashSet::new();
    for (order, node) in graph.nodes.values().enumerate() {
        let eid = node_entity_id(comp, &node.name);
        keep.insert(eid);
        tx.ensure(eid, kinds::COMP_NODE)?;
        tx.link(comp, eid, EdgeKind::Contains)?;
        tx.set(eid, attrs::NAME, node.name.as_str())?;
        tx.set(eid, attrs::ORDER, order as i64)?;
        tx.set(eid, ATTR_KIND, node.kind.as_str())?;
        tx.set(eid, ATTR_PARAMS, Value::from_serde(&node.params)?)?;
        tx.set(
            eid,
            ATTR_POSITION,
            Value::Vec2([node.position[0] as f64, node.position[1] as f64]),
        )?;
        tx.set(eid, ATTR_BYPASS, node.bypass)?;
        tx.set(eid, ATTR_NODE_ID, node.id.to_string())?;
    }
    for child in comp_node_children(tx.graph(), comp) {
        if !keep.contains(&child) {
            tx.delete(child)?;
        }
    }
    tx.set(comp, ATTR_LINKS, Value::from_serde(&graph.links)?)?;
    Ok(())
}

/// Read a comp graph back from the UCG.
pub fn load_graph(graph: &Graph, comp: EntityId) -> Result<CompGraph> {
    require_comp(graph, comp)?;
    let mut out = CompGraph::new();
    for child in comp_node_children(graph, comp) {
        let n = graph.require(child)?;
        let name = n.name();
        let kind = NodeKind::parse(n.get_str(ATTR_KIND).unwrap_or_default())?;
        let params: IndexMap<String, Param> = match n.get(ATTR_PARAMS) {
            Some(v) => v.to_serde()?,
            None => IndexMap::new(),
        };
        let position = match n.get(ATTR_POSITION) {
            Some(Value::Vec2([x, y])) => [*x as f32, *y as f32],
            Some(Value::List(l)) if l.len() == 2 => [
                l[0].as_float().unwrap_or(0.0) as f32,
                l[1].as_float().unwrap_or(0.0) as f32,
            ],
            _ => [0.0, 0.0],
        };
        let id = n
            .get_str(ATTR_NODE_ID)
            .and_then(NodeId::parse)
            .unwrap_or_else(|| NodeId::from_name(&name));
        let mut node = CompNode::new(name, kind);
        node.id = id;
        node.params = params;
        node.position = position;
        node.bypass = n.get_bool(ATTR_BYPASS).unwrap_or(false);
        out.add(node)?;
    }
    if let Some(v) = graph.attr(comp, ATTR_LINKS) {
        let links: Vec<Link> = v.to_serde()?;
        out.links = links
            .into_iter()
            .filter(|l| out.nodes.contains_key(&l.from.0) && out.nodes.contains_key(&l.to.0))
            .collect();
    }
    Ok(out)
}

/// Comp graphs with a direct `DependsOn` edge to `entity`.
pub fn comps_depending_on(graph: &Graph, entity: EntityId) -> Vec<EntityId> {
    graph
        .sources(entity, EdgeKind::DependsOn)
        .into_iter()
        .filter(|id| {
            graph
                .node(*id)
                .map(|n| n.kind.as_str() == kinds::COMP_GRAPH)
                .unwrap_or(false)
        })
        .collect()
}

/// Set the comp's frame range; returns whether it changed.
pub fn update_frame_range(tx: &mut Tx, comp: EntityId, range: TimeRange) -> Result<bool> {
    require_comp(tx.graph(), comp)?;
    let prev = tx
        .graph()
        .attr(comp, attrs::RANGE)
        .and_then(Value::as_range);
    if prev == Some(range) {
        return Ok(false);
    }
    tx.set(comp, attrs::RANGE, range)?;
    Ok(true)
}

/// Flag the comp as stale with a reason; returns whether anything changed.
pub fn mark_dirty(tx: &mut Tx, comp: EntityId, reason: &str) -> Result<bool> {
    require_comp(tx.graph(), comp)?;
    let g = tx.graph();
    let already = g.attr(comp, attrs::STALE).and_then(Value::as_bool) == Some(true)
        && g.attr(comp, attrs::DIRTY_REASON).and_then(Value::as_str) == Some(reason);
    if already {
        return Ok(false);
    }
    tx.set(comp, attrs::STALE, true)?;
    tx.set(comp, attrs::DIRTY_REASON, reason)?;
    Ok(true)
}

/// Clear the stale flag (after a successful render).
pub fn mark_clean(tx: &mut Tx, comp: EntityId) -> Result<bool> {
    require_comp(tx.graph(), comp)?;
    if tx.graph().attr(comp, attrs::STALE).and_then(Value::as_bool) != Some(true) {
        return Ok(false);
    }
    tx.set(comp, attrs::STALE, false)?;
    tx.remove_attr(comp, attrs::DIRTY_REASON)?;
    Ok(true)
}

/// Point every `Read(PrevisPass)` node of the comp at `previs_scene` /
/// `camera` and record `DependsOn` edges to both.
pub fn bind_previs_camera(
    tx: &mut Tx,
    comp: EntityId,
    previs_scene: EntityId,
    camera: EntityId,
) -> Result<()> {
    let mut graph = load_graph(tx.graph(), comp)?;
    for node in graph.nodes.values_mut() {
        if node.kind != NodeKind::Read {
            continue;
        }
        if let ReadSource::PrevisPass { pass, .. } = node.read_source() {
            node.set_read_source(&ReadSource::PrevisPass {
                scene: previs_scene,
                pass,
                camera,
            });
        }
    }
    store_graph(tx, comp, &graph)?;
    if tx.graph().contains(camera) {
        tx.link(comp, camera, EdgeKind::DependsOn)?;
    }
    if tx.graph().contains(previs_scene) {
        tx.link(comp, previs_scene, EdgeKind::DependsOn)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ucg_core::Engine;

    #[test]
    fn template_is_valid() {
        let g = template_graph("SH010");
        assert!(g.validate().is_empty(), "{:?}", g.validate());
        assert_eq!(g.nodes.len(), 7);
        let w = g.by_name("write").unwrap();
        assert_eq!(w.str("path"), "comp/SH010.####.exr");
    }

    #[test]
    fn wrong_kind_rejected() {
        let engine = Engine::new();
        let mut shot = EntityId::nil();
        engine
            .transact("shot", |tx| {
                shot = tx.create(kinds::LOGISTICS_SHOT)?;
                tx.set(shot, attrs::NAME, "SH010")
            })
            .unwrap();
        let g = engine.snapshot();
        assert!(matches!(load_graph(&g, shot), Err(Error::InvalidEntity(_))));
        assert!(load_graph(&g, EntityId::new()).is_err());
    }
}
