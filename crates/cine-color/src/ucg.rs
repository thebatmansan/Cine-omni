//! Integration with the Unified Context Graph: grade entities per clip,
//! color metadata propagation, LUT / config registration and the display
//! processor for a clip.
//!
//! Writers take `&mut Tx`, readers take `&Graph`; every writer is idempotent
//! (a second call with the same inputs produces zero ops).

use crate::error::{Error, Result};
use crate::grade::{cdl_is_identity, GradeStack};
use crate::lut::{parse_cube, Lut, Lut3D};
use crate::ocio::Config;
use crate::processor::{Direction, Op, Processor};
use std::sync::Arc;
use ucg_core::{EdgeKind, EntityId, Graph, Tx, Value};
use ucg_schema::{attrs, kinds, ColorMeta, Component};

/// Attribute holding the serialized [`GradeStack`] on a `color.grade` node.
pub const ATTR_GRADE_STACK: &str = "grade_stack";
/// Attribute holding `.cube` text on a `color.lut` node.
pub const ATTR_CUBE: &str = "cube";
/// Attribute holding OCIO YAML on a `color.config` node.
pub const ATTR_YAML: &str = "yaml";
/// Attribute holding the OCIO family of a `color.space` node.
pub const ATTR_FAMILY: &str = "color.family";
/// Attribute holding the OCIO encoding of a `color.space` node.
pub const ATTR_ENCODING: &str = "color.encoding";
/// Attribute holding the aliases of a `color.space` node.
pub const ATTR_ALIASES: &str = "color.aliases";
/// Attribute marking a `color.space` node as a display colorspace.
pub const ATTR_IS_DISPLAY: &str = "color.is_display";

/// Ensure a `color.grade` entity exists for `clip`.
///
/// The grade id is `EntityId::derived(clip, kinds::COLOR_GRADE)`; it depends
/// on the clip (`DependsOn` edge), carries a [`ColorMeta`] copied from the
/// clip (or defaults) and an empty [`GradeStack`] under `grade_stack`.
/// Idempotent: a second call produces no ops.
pub fn ensure_grade_for_clip(tx: &mut Tx, clip: EntityId) -> Result<EntityId> {
    if !tx.graph().contains(clip) {
        return Err(Error::Ucg(ucg_core::UcgError::NotFound(clip)));
    }
    let id = EntityId::derived(clip, kinds::COLOR_GRADE);
    let created = tx.ensure(id, kinds::COLOR_GRADE)?;
    tx.link(id, clip, EdgeKind::DependsOn)?;
    if created || tx.graph().attr(id, attrs::NAME).is_none() {
        let clip_name = tx.graph().node(clip).map(|n| n.name()).unwrap_or_default();
        tx.set(id, attrs::NAME, format!("grade: {clip_name}"))?;
    }
    if ColorMeta::read_from(tx.graph(), id).is_none() {
        let meta = ColorMeta::read_from(tx.graph(), clip).unwrap_or_default();
        meta.write(tx, id)?;
    }
    if tx.graph().attr(id, ATTR_GRADE_STACK).is_none() {
        tx.set(
            id,
            ATTR_GRADE_STACK,
            Value::from_serde(&GradeStack::default())?,
        )?;
    }
    Ok(id)
}

/// Copy the [`ColorMeta`] component from `from` to `to` when `to` has none or
/// its `input_colorspace` differs. Returns whether anything changed.
pub fn propagate_color_meta(tx: &mut Tx, from: EntityId, to: EntityId) -> Result<bool> {
    let g = tx.graph();
    g.require(from)?;
    g.require(to)?;
    let Some(src) = ColorMeta::read_from(g, from) else {
        return Ok(false);
    };
    let needs = match ColorMeta::read_from(g, to) {
        None => true,
        Some(existing) => existing.input_colorspace != src.input_colorspace,
    };
    if !needs {
        return Ok(false);
    }
    src.write(tx, to)?;
    Ok(true)
}

/// Store a grade stack on a `color.grade` entity.
pub fn store_grade(tx: &mut Tx, grade: EntityId, stack: &GradeStack) -> Result<()> {
    tx.graph().require(grade)?;
    tx.set(grade, ATTR_GRADE_STACK, Value::from_serde(stack)?)?;
    Ok(())
}

/// Load the grade stack of a `color.grade` entity.
pub fn load_grade(graph: &Graph, grade: EntityId) -> Result<GradeStack> {
    let node = graph.require(grade)?;
    let v = node.get(ATTR_GRADE_STACK).ok_or(Error::NoGrade(grade))?;
    Ok(v.to_serde()?)
}

/// All `color.grade` entities depending on `clip`.
pub fn grades_for(graph: &Graph, clip: EntityId) -> Vec<EntityId> {
    graph
        .sources(clip, EdgeKind::DependsOn)
        .into_iter()
        .filter(|id| {
            graph
                .node(*id)
                .map(|n| n.kind.as_str() == kinds::COLOR_GRADE)
                .unwrap_or(false)
        })
        .collect()
}

/// Register a 3D LUT under `project` as a `color.lut` entity
/// (id `from_key("lut:{name}")`, `.cube` text in `cube`).
pub fn register_lut(tx: &mut Tx, project: EntityId, name: &str, lut: &Lut3D) -> Result<EntityId> {
    tx.graph().require(project)?;
    let id = EntityId::from_key(&format!("lut:{name}"));
    tx.ensure(id, kinds::COLOR_LUT)?;
    tx.set(id, attrs::NAME, name)?;
    tx.set(id, ATTR_CUBE, lut.to_cube())?;
    tx.set(id, "size", lut.size as i64)?;
    tx.link(project, id, EdgeKind::Contains)?;
    Ok(id)
}

/// Load a registered LUT.
pub fn load_lut(graph: &Graph, lut: EntityId) -> Result<Lut3D> {
    let node = graph.require(lut)?;
    let text = node
        .get_str(ATTR_CUBE)
        .ok_or_else(|| Error::Lut(format!("entity {lut} has no cube text")))?;
    match parse_cube(text)? {
        Lut::ThreeD(l) => Ok(l),
        Lut::OneD(_) => Err(Error::Lut("expected a 3D LUT".into())),
    }
}

/// Install an OCIO config under `project`: a `color.config` node holding the
/// YAML plus one `color.space` child per colorspace. Idempotent.
pub fn install_config(tx: &mut Tx, project: EntityId, config: &Config) -> Result<EntityId> {
    tx.graph().require(project)?;
    let id = EntityId::derived(project, kinds::COLOR_CONFIG);
    tx.ensure(id, kinds::COLOR_CONFIG)?;
    let name = if config.name.is_empty() {
        "ocio config".to_string()
    } else {
        config.name.clone()
    };
    tx.set(id, attrs::NAME, name)?;
    tx.set(id, ATTR_YAML, config.to_yaml())?;
    tx.set(id, "version", config.version.as_str())?;
    tx.link(project, id, EdgeKind::Contains)?;
    // Colorspace children (stable ids per config + name); remove stale ones.
    let wanted: Vec<EntityId> = config
        .colorspaces
        .iter()
        .map(|c| EntityId::derived(id, &format!("{}:{}", kinds::COLOR_SPACE, c.name)))
        .collect();
    for child in tx.graph().children(id) {
        if !wanted.contains(&child) {
            tx.delete(child)?;
        }
    }
    for (i, cs) in config.colorspaces.iter().enumerate() {
        let cid = wanted[i];
        tx.ensure(cid, kinds::COLOR_SPACE)?;
        tx.set(cid, attrs::NAME, cs.name.as_str())?;
        tx.set(cid, attrs::ORDER, i as i64)?;
        tx.set(cid, ATTR_FAMILY, cs.family.as_str())?;
        tx.set(cid, ATTR_ENCODING, cs.encoding.as_str())?;
        tx.set(cid, ATTR_ALIASES, cs.aliases.clone())?;
        tx.set(cid, ATTR_IS_DISPLAY, cs.is_display)?;
        tx.link(id, cid, EdgeKind::Contains)?;
    }
    Ok(id)
}

/// Load the config stored by [`install_config`].
pub fn load_config(graph: &Graph, config: EntityId) -> Result<Config> {
    let node = graph.require(config)?;
    let text = node
        .get_str(ATTR_YAML)
        .ok_or_else(|| Error::Invalid(format!("entity {config} has no yaml")))?;
    Config::parse(text)
}

/// The full display processor for a clip from its [`ColorMeta`]:
/// `input_colorspace → working_colorspace`, the CDL and look LUT (if any) in
/// the working space, then `working → display`. `display` may be a display
/// name of the config (its first active view is used) or a colorspace name.
pub fn processor_for_clip(graph: &Graph, clip: EntityId, config: &Config) -> Result<Processor> {
    graph.require(clip)?;
    let meta = ColorMeta::read_from(graph, clip).ok_or(Error::NoColorMeta(clip))?;
    let mut ops: Vec<Op> = Vec::new();
    ops.extend(
        config
            .processor(&meta.input_colorspace, &meta.working_colorspace)?
            .ops()
            .iter()
            .cloned(),
    );
    if !cdl_is_identity(&meta.cdl) {
        ops.push(Op::Cdl {
            cdl: meta.cdl.clone(),
            direction: Direction::Forward,
        });
    }
    if let Some(lut_id) = meta.look_lut {
        let lut = load_lut(graph, lut_id)?;
        ops.push(Op::Lut3D(Arc::new(lut)));
    }
    let to_display = if let Ok(views) = config.views(&meta.display) {
        let view = config
            .active_views
            .iter()
            .find(|v| views.iter().any(|x| x.name.eq_ignore_ascii_case(v)))
            .cloned()
            .or_else(|| views.first().map(|v| v.name.clone()))
            .ok_or_else(|| Error::UnknownView {
                display: meta.display.clone(),
                view: String::new(),
            })?;
        config.display_processor(&meta.working_colorspace, &meta.display, &view)?
    } else {
        config.processor(&meta.working_colorspace, &meta.display)?
    };
    ops.extend(to_display.ops().iter().cloned());
    Ok(Processor::new(ops))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ucg_core::Engine;

    #[test]
    fn ensure_grade_is_idempotent() {
        let engine = Engine::new();
        let mut clip = EntityId::nil();
        engine
            .transact("clip", |tx| {
                clip = tx.create(kinds::NLE_CLIP)?;
                tx.set(clip, attrs::NAME, "A001_C002")?;
                ColorMeta {
                    input_colorspace: "ARRI LogC4".into(),
                    ..Default::default()
                }
                .write(tx, clip)
            })
            .unwrap();
        let mut grade = EntityId::nil();
        let g1 = engine
            .transact("grade", |tx| {
                grade = ensure_grade_for_clip(tx, clip)?;
                Ok(())
            })
            .unwrap();
        assert!(!g1.deltas.is_empty());
        let g2 = engine
            .transact("grade again", |tx| {
                let again = ensure_grade_for_clip(tx, clip)?;
                assert_eq!(again, grade);
                Ok(())
            })
            .unwrap();
        assert!(g2.deltas.is_empty(), "second call must produce zero ops");
        let g = engine.read();
        assert_eq!(grade, EntityId::derived(clip, kinds::COLOR_GRADE));
        assert!(g.has_edge(grade, clip, EdgeKind::DependsOn));
        assert_eq!(
            ColorMeta::read_from(&g, grade).unwrap().input_colorspace,
            "ARRI LogC4"
        );
        assert!(load_grade(&g, grade).unwrap().is_empty());
        assert_eq!(grades_for(&g, clip), vec![grade]);
    }

    #[test]
    fn propagate_semantics() {
        let engine = Engine::new();
        let (mut asset, mut clip) = (EntityId::nil(), EntityId::nil());
        engine
            .transact("setup", |tx| {
                asset = tx.create(kinds::ASSET_MEDIA)?;
                clip = tx.create(kinds::NLE_CLIP)?;
                ColorMeta {
                    input_colorspace: "Sony S-Log3 S-Gamut3.Cine".into(),
                    ..Default::default()
                }
                .write(tx, asset)
            })
            .unwrap();
        engine
            .transact("p1", |tx| {
                assert!(propagate_color_meta(tx, asset, clip)?);
                assert!(!propagate_color_meta(tx, asset, clip)?);
                Ok(())
            })
            .unwrap();
        // same input colorspace but different display → no change
        engine
            .transact("p2", |tx| {
                let mut m = ColorMeta::read_from(tx.graph(), clip).unwrap();
                m.display = "Rec.2100-PQ".into();
                m.write(tx, clip)?;
                assert!(!propagate_color_meta(tx, asset, clip)?);
                Ok(())
            })
            .unwrap();
        // changed input colorspace on the asset → propagates
        engine
            .transact("p3", |tx| {
                ColorMeta {
                    input_colorspace: "ARRI LogC4".into(),
                    ..Default::default()
                }
                .write(tx, asset)?;
                assert!(propagate_color_meta(tx, asset, clip)?);
                Ok(())
            })
            .unwrap();
        let g = engine.read();
        assert_eq!(
            ColorMeta::read_from(&g, clip).unwrap().input_colorspace,
            "ARRI LogC4"
        );
        // source without metadata → false
        let engine2 = Engine::new();
        engine2
            .transact("x", |tx| {
                let a = tx.create(kinds::ASSET_MEDIA)?;
                let b = tx.create(kinds::NLE_CLIP)?;
                assert!(!propagate_color_meta(tx, a, b)?);
                Ok(())
            })
            .unwrap();
    }
}
