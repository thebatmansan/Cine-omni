//! The colourist's grade: what the project holds, and how it is changed.
//!
//! A `color.grade` node has carried a [`GradeStack`] — layers of operations,
//! with masks and qualifiers — since the colour engine was built. Until now
//! nothing read it into a picture: the viewer evaluated the ASC CDL on the
//! clip's [`ColorMeta`] and stopped there, so the richer model was stored,
//! round-tripped by one test, and never seen. This module is the protocol side
//! of giving it a path.
//!
//! ## Two things, not two models
//!
//! Both live in the working space, and they are different things:
//!
//! * the **CDL** is the value that came from the set or the dailies, and it is
//!   what a facility exchanges — it round-trips to and from CDL XML;
//! * the **grade** is the work done on top of it in the suite.
//!
//! They are applied in that order, which is the order a film is actually made.
//! `set_grade` still writes the CDL and means the CDL; this writes the grade.
//!
//! ## What is exposed, and what is not
//!
//! The engine evaluates eleven operations. This exposes the five that make up
//! a primary grade and need nothing but numbers:
//! [`Exposure`](GradeOp::Exposure), [`Contrast`](GradeOp::Contrast),
//! [`Saturation`](GradeOp::Saturation),
//! [`TemperatureTint`](GradeOp::TemperatureTint) and
//! [`PrinterLights`](GradeOp::PrinterLights).
//!
//! Wheels, curves, 3D LUTs, matrices, masks and HSL qualifiers are **in the
//! engine and not on this protocol**. They need an interface that is not a
//! number — a wheel, a curve editor, a shape tool — and a control that cannot
//! be driven is worse than one that is absent. [`unsupported`] names them, so
//! a client can say what exists rather than discovering it by failing.
//!
//! Masks go further than "no interface". A colour transform maps a colour to a
//! colour and is never told where the pixel was, so a masked grade cannot be
//! carried by the view chain at all: `cine_color::ucg::viewing_processor_with`
//! refuses one by name. A grade written elsewhere with a mask on it is
//! therefore visible in [`read`] — the layer is listed and marked `masked` —
//! and unviewable, which is the honest pair of facts.

use cine_color::grade::{GradeLayer, GradeOp, GradeStack};
use cine_pipeline::OmniProject;
use serde_json::{json, Value as J};
use ucg_core::{EntityId, UcgError};
use ucg_schema::kinds;

/// The operations this protocol can read and write.
pub const SUPPORTED: &[&str] = &[
    "exposure",
    "contrast",
    "saturation",
    "temperature_tint",
    "printer_lights",
];

/// Operations the engine evaluates that this protocol does not yet carry, and
/// why. Reported so an interface can state the limit instead of implying none.
pub fn unsupported() -> J {
    json!([
        { "op": "wheels", "reason": "lift, gamma, gain and offset need colour wheels, not numbers" },
        { "op": "cdl", "reason": "the ASC CDL is its own value on the clip; set it with `set_grade`" },
        { "op": "hue_rotate", "reason": "no interface for it yet" },
        { "op": "curves", "reason": "needs a curve editor" },
        { "op": "lut3d", "reason": "needs look management: importing, naming and versioning a LUT" },
        { "op": "matrix", "reason": "no interface for a free matrix yet" },
        { "op": "mask", "reason": "needs a shape tool, and a masked grade cannot be baked into a lattice" },
        { "op": "qualifier", "reason": "needs an HSL picker" },
    ])
}

/// One operation as the protocol carries it.
fn op_json(op: &GradeOp) -> J {
    match op {
        GradeOp::Exposure { stops } => json!({ "op": "exposure", "stops": stops }),
        GradeOp::Contrast { amount, pivot } => {
            json!({ "op": "contrast", "amount": amount, "pivot": pivot })
        }
        GradeOp::Saturation { amount } => json!({ "op": "saturation", "amount": amount }),
        GradeOp::TemperatureTint {
            temperature_k,
            tint,
        } => json!({ "op": "temperature_tint", "temperature_k": temperature_k, "tint": tint }),
        GradeOp::PrinterLights { points } => json!({ "op": "printer_lights", "points": points }),
        // Present in the grade, not editable here. Named rather than hidden:
        // an interface must be able to say "this grade has a curve on it that
        // you cannot change from this build" instead of showing a grade that
        // is missing part of itself.
        other => json!({ "op": other.label(), "editable": false }),
    }
}

fn finite(v: Option<&J>, what: &str) -> Result<f64, String> {
    v.and_then(J::as_f64)
        .filter(|f| f.is_finite())
        .ok_or_else(|| format!("`{what}` must be a finite number"))
}

/// Read one operation from the protocol, refusing what the engine cannot mean.
pub fn op_from_json(v: &J) -> Result<GradeOp, String> {
    let name = v
        .get("op")
        .and_then(J::as_str)
        .ok_or("every operation needs an `op`")?;
    Ok(match name {
        "exposure" => GradeOp::Exposure {
            stops: finite(v.get("stops"), "stops")?,
        },
        "contrast" => {
            let pivot = match v.get("pivot") {
                None | Some(J::Null) => 0.18,
                other => finite(other, "pivot")?,
            };
            if pivot <= 0.0 {
                return Err("`pivot` must be greater than zero: contrast turns about it".into());
            }
            GradeOp::Contrast {
                amount: finite(v.get("amount"), "amount")?,
                pivot,
            }
        }
        "saturation" => {
            let amount = finite(v.get("amount"), "amount")?;
            if amount < 0.0 {
                return Err(
                    "`amount` must be 0 or more: negative saturation is not a grade".into(),
                );
            }
            GradeOp::Saturation { amount }
        }
        "temperature_tint" => GradeOp::TemperatureTint {
            temperature_k: finite(v.get("temperature_k"), "temperature_k")?,
            tint: finite(v.get("tint"), "tint")?,
        },
        "printer_lights" => {
            let list = v
                .get("points")
                .and_then(J::as_array)
                .filter(|l| l.len() == 3)
                .ok_or("`points` must be three numbers")?;
            let mut points = [0.0; 3];
            for (i, p) in list.iter().enumerate() {
                points[i] = finite(Some(p), "points")?;
            }
            GradeOp::PrinterLights { points }
        }
        other if SUPPORTED.contains(&other) => {
            return Err(format!("`{other}` is not readable from this protocol"))
        }
        other => {
            return Err(format!(
                "`{other}` is not an operation this build can set. This protocol carries {}. \
                 Others exist in the engine and have no interface yet.",
                SUPPORTED.join(", ")
            ))
        }
    })
}

/// The grade of a clip, as the interface reads it.
pub fn read(project: &OmniProject, clip: EntityId) -> Result<J, String> {
    let g = project.engine.read();
    if !g
        .node(clip)
        .is_some_and(|n| n.kind.as_str() == kinds::NLE_CLIP)
    {
        return Err(format!("{clip} is not a clip in this project"));
    }
    let grade = EntityId::derived(clip, kinds::COLOR_GRADE);
    // Every placed clip already has a `color.grade` node: a reactor makes one
    // and propagates the clip's colour metadata onto it, so the node exists
    // long before anybody grades anything. The question an interface actually
    // has is not whether the node is there but whether there is anything in
    // it, and those are reported separately. Nothing is created by looking.
    let exists = g.node(grade).is_some();
    let stack = if exists {
        cine_color::ucg::load_grade(&g, grade).unwrap_or_default()
    } else {
        GradeStack::default()
    };
    let cdl = cine_color::ucg::grade_of_clip(&g, clip);
    let graded = !stack.is_empty() || cdl.is_some();
    // Whether `set_primary` would be accepted, said here so an interface can
    // disable the controls with the reason rather than find out by failing.
    let primary_why_not = primary_why_not(&stack);
    Ok(json!({
        "clip": clip,
        "grade": exists.then_some(grade),
        // The node is there (a reactor makes one per clip)...
        "exists": exists,
        // ...and this says whether anything in it would change a picture.
        "graded": graded,
        "working_primaries": format!("{:?}", stack.working_primaries),
        "layers": stack.layers.iter().map(|l| json!({
            "name": l.name,
            "enabled": l.enabled,
            "opacity": l.opacity,
            "masked": l.mask.is_some(),
            "qualified": l.qualifier.is_some(),
            "ops": l.ops.iter().map(op_json).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        // The CDL is its own value, shown beside the grade rather than inside
        // it, because that is what it is.
        "cdl": cdl.map(|c| json!({
            "slope": c.slope, "offset": c.offset, "power": c.power, "saturation": c.saturation,
        })),
        "supported": SUPPORTED,
        "unsupported": unsupported(),
        // The primary controls are offered only when writing one is safe.
        "primary_settable": primary_why_not.is_none(),
        "primary_why_not": primary_why_not,
    }))
}

/// Why this stack's first layer cannot be written as a primary from here, when
/// it cannot.
///
/// One implementation, reported by [`read`] and enforced by [`set_primary`], so
/// what an interface is told and what it is allowed to do are the same answer.
/// An operation this protocol cannot express would be destroyed by the write,
/// and a mask or a qualifier would silently confine the numbers somebody typed
/// to part of the frame. Either is a refusal, not a warning.
fn primary_why_not(stack: &GradeStack) -> Option<String> {
    let first = stack.layers.first()?;
    if let Some(op) = first.ops.iter().find(|o| !SUPPORTED.contains(&o.label())) {
        return Some(format!(
            "this clip's first grade layer holds a `{}` operation this protocol cannot express, so writing a primary here would discard it",
            op.label()
        ));
    }
    (first.mask.is_some() || first.qualifier.is_some()).then(|| {
        "this clip's first grade layer is masked or qualified, so a primary written into it would apply to part of the frame only".to_string()
    })
}

/// Replace the operations of a clip's primary grade layer.
///
/// One layer, because that is what this protocol exposes; the engine's stack
/// is a list and the rest of it is reachable when there is an interface for
/// it. Setting an empty list leaves a grade that changes nothing, which is
/// different from having no grade — and is what an undo of the first edit
/// produces.
///
/// Refused rather than attempted when the first layer is not a primary this
/// protocol can stand in for: an operation it cannot express would be
/// destroyed by the write, and a mask or a qualifier would silently confine
/// the numbers somebody typed to part of the frame. Other layers are untouched
/// either way.
pub fn set_primary(project: &OmniProject, clip: EntityId, ops: Vec<GradeOp>) -> Result<J, String> {
    {
        let g = project.engine.read();
        if !g
            .node(clip)
            .is_some_and(|n| n.kind.as_str() == kinds::NLE_CLIP)
        {
            return Err(format!("{clip} is not a clip in this project"));
        }
        let existing = cine_color::ucg::load_grade(&g, EntityId::derived(clip, kinds::COLOR_GRADE))
            .unwrap_or_default();
        if let Some(why) = primary_why_not(&existing) {
            return Err(why);
        }
    }
    let group = project
        .engine
        .transact("grade", |tx| {
            let grade = cine_color::ucg::ensure_grade_for_clip(tx, clip)
                .map_err(|e| UcgError::Aborted(e.to_string()))?;
            let mut stack = cine_color::ucg::load_grade(tx.graph(), grade).unwrap_or_default();
            // Only the first layer is written; the rest of the stack is left
            // exactly as it is. That the first layer is one this protocol can
            // speak for was established above, before the write.
            match stack.layers.first_mut() {
                Some(first) => first.ops = ops,
                None => stack.layers.push(GradeLayer {
                    name: "primary".into(),
                    ops,
                    ..Default::default()
                }),
            }
            cine_color::ucg::store_grade(tx, grade, &stack)
                .map_err(|e| UcgError::Aborted(e.to_string()))
        })
        .map_err(|e| e.to_string())?;
    let _ = group;
    read(project, clip)
}
