//! A USD-style scene description: a hierarchy of typed prims with
//! transforms and attribute bags, plus USDA text export and a minimal
//! parser that reads back what the exporter writes.
//!
//! The stage is deliberately small: it is the interchange surface between
//! the graph (see [`crate::graph::stage_from_graph`]), the viewport and
//! external DCCs. It is **not** a general USD implementation — composition
//! arcs, variants, time samples and schemas beyond the handful used here
//! are out of scope.
//!
//! Conventions: metres, Y up, camera apertures in millimetres (written to
//! `focalLength` / `horizontalAperture` as-is, which matches what most DCC
//! exporters do despite USD's "tenths of scene unit" wording).

use glam::{DMat4, DQuat, DVec3};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as Json};
use std::fmt;
use ucg_schema::{Bounds, CameraParams, LightParams, LightType, Transform};

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Whether `s` is a valid USD identifier (`[A-Za-z_][A-Za-z0-9_]*`).
pub fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Turn an arbitrary display name (`"Dr. Ana Ruiz-Ortega"`) into a valid
/// identifier (`"Dr_Ana_Ruiz_Ortega"`). Empty input becomes `"_"`.
pub fn sanitize_identifier(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_us = false;
    for c in s.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_us = false;
        } else if !last_us {
            out.push('_');
            last_us = true;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        return "_".to_string();
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("_{out}")
    } else {
        out
    }
}

/// An absolute prim path such as `/World/Cameras/CamA`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PrimPath(String);

impl PrimPath {
    /// The pseudo-root `/`.
    pub fn root() -> Self {
        PrimPath("/".to_string())
    }

    /// Parse and validate an absolute path.
    pub fn new(s: &str) -> Result<Self> {
        if s == "/" {
            return Ok(Self::root());
        }
        if !s.starts_with('/') {
            return Err(Error::InvalidPrimPath(
                s.to_string(),
                "must be absolute (start with '/')",
            ));
        }
        if s.ends_with('/') {
            return Err(Error::InvalidPrimPath(
                s.to_string(),
                "must not end with '/'",
            ));
        }
        for seg in s[1..].split('/') {
            if !is_valid_identifier(seg) {
                return Err(Error::InvalidPrimPath(
                    s.to_string(),
                    "segments must be identifiers",
                ));
            }
        }
        Ok(PrimPath(s.to_string()))
    }

    /// The path as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Whether this is `/`.
    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }
    /// Number of segments (`/` has depth 0).
    pub fn depth(&self) -> usize {
        self.components().len()
    }
    /// Path segments.
    pub fn components(&self) -> Vec<&str> {
        if self.is_root() {
            Vec::new()
        } else {
            self.0[1..].split('/').collect()
        }
    }
    /// The last segment (`""` for the root).
    pub fn name(&self) -> &str {
        if self.is_root() {
            ""
        } else {
            self.0.rsplit('/').next().unwrap_or("")
        }
    }
    /// Parent path (`None` for the root).
    pub fn parent(&self) -> Option<PrimPath> {
        if self.is_root() {
            return None;
        }
        match self.0.rfind('/') {
            Some(0) => Some(Self::root()),
            Some(i) => Some(PrimPath(self.0[..i].to_string())),
            None => None,
        }
    }
    /// Append a child segment.
    pub fn child(&self, name: &str) -> Result<PrimPath> {
        if !is_valid_identifier(name) {
            return Err(Error::InvalidPrimPath(
                format!("{}/{}", self.0, name),
                "child name must be an identifier",
            ));
        }
        if self.is_root() {
            Ok(PrimPath(format!("/{name}")))
        } else {
            Ok(PrimPath(format!("{}/{}", self.0, name)))
        }
    }
    /// Whether `self` is a strict ancestor of `other`.
    pub fn is_ancestor_of(&self, other: &PrimPath) -> bool {
        if self == other {
            return false;
        }
        if self.is_root() {
            return true;
        }
        other.0.starts_with(&self.0) && other.0.as_bytes().get(self.0.len()) == Some(&b'/')
    }
}

impl fmt::Debug for PrimPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{}>", self.0)
    }
}
impl fmt::Display for PrimPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Prims
// ---------------------------------------------------------------------------

/// Prim schema types supported by the stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PrimType {
    /// Plain transform group.
    #[default]
    Xform,
    /// Physical camera (see [`Prim::set_camera`]).
    Camera,
    /// Light of any kind (see [`Prim::set_light`]).
    Light,
    /// Polygon mesh (geometry lives in attrs or external assets).
    Mesh,
    /// Non-transformable grouping node.
    Scope,
    /// Character stand-in (an Xform with `extent` and `cine:height`).
    Character,
    /// Ground plane / set floor.
    Plane,
}

impl PrimType {
    /// Whether prims of this type carry xformOps in USDA.
    pub fn is_xformable(self) -> bool {
        !matches!(self, PrimType::Scope)
    }
}

/// Reserved attribute names the exporter derives from typed fields and the
/// parser folds back into them.
const RESERVED_ATTRS: &[&str] = &[
    "xformOp:translate",
    "xformOp:orient",
    "xformOp:scale",
    "xformOpOrder",
    "cine:primType",
];

/// A prim: a node of the stage hierarchy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prim {
    /// Absolute path.
    pub path: PrimPath,
    /// Schema type.
    pub prim_type: PrimType,
    /// Local transform relative to the parent.
    pub transform: Transform,
    /// Attribute bag (USD attribute names where a USD equivalent exists).
    pub attrs: IndexMap<String, Json>,
    /// Child paths in definition order.
    pub children: Vec<PrimPath>,
}

impl Prim {
    /// A prim with an identity transform and no attributes.
    pub fn new(path: PrimPath, prim_type: PrimType) -> Self {
        Prim {
            path,
            prim_type,
            transform: Transform::default(),
            attrs: IndexMap::new(),
            children: Vec::new(),
        }
    }
    /// The prim's name (last path segment).
    pub fn name(&self) -> &str {
        self.path.name()
    }
    /// Set an attribute.
    pub fn set_attr(&mut self, key: impl Into<String>, value: Json) -> &mut Self {
        self.attrs.insert(key.into(), value);
        self
    }
    /// Read an attribute.
    pub fn attr(&self, key: &str) -> Option<&Json> {
        self.attrs.get(key)
    }
    /// Read a numeric attribute.
    pub fn attr_f64(&self, key: &str) -> Option<f64> {
        self.attrs.get(key).and_then(Json::as_f64)
    }
    /// Read a string attribute.
    pub fn attr_str(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).and_then(Json::as_str)
    }

    /// Set the local `extent` (axis-aligned, in local units).
    pub fn set_extent(&mut self, min: [f64; 3], max: [f64; 3]) -> &mut Self {
        self.set_attr("extent", json!([min, max]))
    }
    /// Local extent, if any.
    pub fn extent(&self) -> Option<([f64; 3], [f64; 3])> {
        let e = self.attrs.get("extent")?.as_array()?;
        if e.len() != 2 {
            return None;
        }
        Some((json_vec3(&e[0])?, json_vec3(&e[1])?))
    }

    /// Write camera parameters as USD camera attributes (plus `cine:*`
    /// extras for the values USD has no slot for).
    pub fn set_camera(&mut self, p: &CameraParams) -> &mut Self {
        self.set_attr("projection", json!("perspective"));
        self.set_attr("focalLength", json!(p.focal_length_mm));
        self.set_attr("horizontalAperture", json!(p.sensor_width_mm));
        self.set_attr("verticalAperture", json!(p.sensor_height_mm));
        self.set_attr("fStop", json!(p.f_stop));
        self.set_attr("focusDistance", json!(p.focus_distance_m));
        self.set_attr("clippingRange", json!([p.clip[0], p.clip[1]]));
        self.set_attr("cine:iso", json!(p.iso));
        self.set_attr("cine:shutterAngle", json!(p.shutter_angle_deg));
        self.set_attr("cine:whiteBalance", json!(p.white_balance_k));
        self.set_attr("cine:frameRate", json!(p.frame_rate));
        self.set_attr(
            "cine:resolution",
            json!([p.resolution[0] as f64, p.resolution[1] as f64]),
        );
        self
    }

    /// Read camera parameters back (missing values fall back to defaults).
    pub fn camera_params(&self) -> Option<CameraParams> {
        if self.prim_type != PrimType::Camera {
            return None;
        }
        let d = CameraParams::default();
        let clip = self
            .attrs
            .get("clippingRange")
            .and_then(json_vec2)
            .unwrap_or(d.clip);
        let res = self
            .attrs
            .get("cine:resolution")
            .and_then(json_vec2)
            .unwrap_or([d.resolution[0] as f64, d.resolution[1] as f64]);
        Some(CameraParams {
            sensor_width_mm: self
                .attr_f64("horizontalAperture")
                .unwrap_or(d.sensor_width_mm),
            sensor_height_mm: self
                .attr_f64("verticalAperture")
                .unwrap_or(d.sensor_height_mm),
            focal_length_mm: self.attr_f64("focalLength").unwrap_or(d.focal_length_mm),
            f_stop: self.attr_f64("fStop").unwrap_or(d.f_stop),
            focus_distance_m: self.attr_f64("focusDistance").unwrap_or(d.focus_distance_m),
            shutter_angle_deg: self
                .attr_f64("cine:shutterAngle")
                .unwrap_or(d.shutter_angle_deg),
            iso: self.attr_f64("cine:iso").unwrap_or(d.iso),
            white_balance_k: self
                .attr_f64("cine:whiteBalance")
                .unwrap_or(d.white_balance_k),
            frame_rate: self.attr_f64("cine:frameRate").unwrap_or(d.frame_rate),
            resolution: [res[0].max(1.0) as u32, res[1].max(1.0) as u32],
            clip,
        })
    }

    /// Write light parameters as UsdLux attributes plus `cine:light:*` extras.
    pub fn set_light(&mut self, l: &LightParams) -> &mut Self {
        self.set_attr("inputs:intensity", json!(l.intensity));
        self.set_attr("inputs:colorTemperature", json!(l.color_temp_k));
        self.set_attr("inputs:enableColorTemperature", json!(true));
        self.set_attr("inputs:color", json!(l.tint));
        match l.kind {
            LightType::Point => {
                self.set_attr("inputs:radius", json!(l.size_m[0] * 0.5));
            }
            LightType::Spot => {
                self.set_attr("inputs:radius", json!(l.size_m[0] * 0.5));
                self.set_attr("inputs:shaping:cone:angle", json!(l.cone_angle_deg * 0.5));
                self.set_attr("inputs:shaping:cone:softness", json!(l.softness));
            }
            LightType::Area => {
                self.set_attr("inputs:width", json!(l.size_m[0]));
                self.set_attr("inputs:height", json!(l.size_m[1]));
            }
            LightType::Directional => {
                self.set_attr("inputs:angle", json!(0.53));
            }
            LightType::Dome => {}
        }
        self.set_attr("cine:light:kind", json!(light_kind_name(l.kind)));
        self.set_attr("cine:light:coneAngle", json!(l.cone_angle_deg));
        self.set_attr("cine:light:softness", json!(l.softness));
        self.set_attr("cine:light:size", json!(l.size_m));
        self
    }

    /// Read light parameters back (`cine:light:*` first, UsdLux fallback).
    pub fn light_params(&self) -> Option<LightParams> {
        if self.prim_type != PrimType::Light {
            return None;
        }
        let d = LightParams::default();
        let kind = self
            .attr_str("cine:light:kind")
            .and_then(light_kind_from_name)
            .unwrap_or(d.kind);
        let size = self
            .attrs
            .get("cine:light:size")
            .and_then(json_vec2)
            .unwrap_or_else(|| {
                let w = self
                    .attr_f64("inputs:width")
                    .or_else(|| self.attr_f64("inputs:radius").map(|r| r * 2.0));
                let h = self.attr_f64("inputs:height").or(w);
                [w.unwrap_or(d.size_m[0]), h.unwrap_or(d.size_m[1])]
            });
        let cone = self
            .attr_f64("cine:light:coneAngle")
            .or_else(|| self.attr_f64("inputs:shaping:cone:angle").map(|a| a * 2.0))
            .unwrap_or(d.cone_angle_deg);
        let softness = self
            .attr_f64("cine:light:softness")
            .or_else(|| self.attr_f64("inputs:shaping:cone:softness"))
            .unwrap_or(d.softness);
        Some(LightParams {
            kind,
            intensity: self.attr_f64("inputs:intensity").unwrap_or(d.intensity),
            color_temp_k: self
                .attr_f64("inputs:colorTemperature")
                .unwrap_or(d.color_temp_k),
            tint: self
                .attrs
                .get("inputs:color")
                .and_then(json_vec3)
                .unwrap_or(d.tint),
            cone_angle_deg: cone,
            softness,
            size_m: size,
        })
    }

    /// USD schema type name used in `def <Type> "name"`.
    pub fn usd_type_name(&self) -> &'static str {
        match self.prim_type {
            PrimType::Xform | PrimType::Character => "Xform",
            PrimType::Camera => "Camera",
            PrimType::Mesh => "Mesh",
            PrimType::Scope => "Scope",
            PrimType::Plane => "Plane",
            PrimType::Light => {
                let kind = self
                    .attr_str("cine:light:kind")
                    .and_then(light_kind_from_name)
                    .unwrap_or_default();
                match kind {
                    LightType::Point | LightType::Spot => "SphereLight",
                    LightType::Area => "RectLight",
                    LightType::Directional => "DistantLight",
                    LightType::Dome => "DomeLight",
                }
            }
        }
    }
}

fn light_kind_name(k: LightType) -> &'static str {
    match k {
        LightType::Point => "point",
        LightType::Spot => "spot",
        LightType::Area => "area",
        LightType::Directional => "directional",
        LightType::Dome => "dome",
    }
}

fn light_kind_from_name(s: &str) -> Option<LightType> {
    Some(match s {
        "point" => LightType::Point,
        "spot" => LightType::Spot,
        "area" => LightType::Area,
        "directional" => LightType::Directional,
        "dome" => LightType::Dome,
        _ => return None,
    })
}

fn json_vec2(v: &Json) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    if a.len() != 2 {
        return None;
    }
    Some([a[0].as_f64()?, a[1].as_f64()?])
}

fn json_vec3(v: &Json) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?])
}

fn json_vec4(v: &Json) -> Option<[f64; 4]> {
    let a = v.as_array()?;
    if a.len() != 4 {
        return None;
    }
    Some([
        a[0].as_f64()?,
        a[1].as_f64()?,
        a[2].as_f64()?,
        a[3].as_f64()?,
    ])
}

// ---------------------------------------------------------------------------
// Stage
// ---------------------------------------------------------------------------

/// A scene: prims keyed by path in definition order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    /// All prims (excluding the pseudo-root), keyed by absolute path.
    pub prims: IndexMap<PrimPath, Prim>,
    /// Scene unit scale (1 = metres).
    pub meters_per_unit: f64,
    /// `"Y"` or `"Z"`.
    pub up_axis: String,
    /// Layer default prim (first top-level prim when unset).
    pub default_prim: Option<String>,
}

impl Default for Stage {
    fn default() -> Self {
        Stage {
            prims: IndexMap::new(),
            meters_per_unit: 1.0,
            up_axis: "Y".to_string(),
            default_prim: None,
        }
    }
}

impl Stage {
    /// An empty stage (metres, Y up).
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of prims.
    pub fn len(&self) -> usize {
        self.prims.len()
    }
    /// Whether there are no prims.
    pub fn is_empty(&self) -> bool {
        self.prims.is_empty()
    }
    /// Whether a prim exists at `path`.
    pub fn contains(&self, path: &PrimPath) -> bool {
        self.prims.contains_key(path)
    }
    /// Look up a prim.
    pub fn get(&self, path: &PrimPath) -> Option<&Prim> {
        self.prims.get(path)
    }
    /// Look up a prim mutably.
    pub fn get_mut(&mut self, path: &PrimPath) -> Option<&mut Prim> {
        self.prims.get_mut(path)
    }
    /// Look up a prim or fail.
    pub fn require(&self, path: &PrimPath) -> Result<&Prim> {
        self.prims
            .get(path)
            .ok_or_else(|| Error::PrimNotFound(path.to_string()))
    }
    /// Iterate prims in definition order.
    pub fn iter(&self) -> impl Iterator<Item = &Prim> {
        self.prims.values()
    }
    /// Top-level prims (children of `/`).
    pub fn roots(&self) -> Vec<&Prim> {
        self.prims
            .values()
            .filter(|p| p.path.depth() == 1)
            .collect()
    }
    /// Children of a prim, in order.
    pub fn children_of(&self, path: &PrimPath) -> Vec<&Prim> {
        if path.is_root() {
            return self.roots();
        }
        self.prims
            .get(path)
            .map(|p| {
                p.children
                    .iter()
                    .filter_map(|c| self.prims.get(c))
                    .collect()
            })
            .unwrap_or_default()
    }
    /// All prims of a type.
    pub fn of_type(&self, t: PrimType) -> Vec<&Prim> {
        self.prims.values().filter(|p| p.prim_type == t).collect()
    }
    /// Camera prims.
    pub fn cameras(&self) -> Vec<&Prim> {
        self.of_type(PrimType::Camera)
    }
    /// Light prims.
    pub fn lights(&self) -> Vec<&Prim> {
        self.of_type(PrimType::Light)
    }
    /// Character stand-ins.
    pub fn characters(&self) -> Vec<&Prim> {
        self.of_type(PrimType::Character)
    }

    /// Define a prim (creating missing ancestors as `Xform`), or return the
    /// existing prim at that path. The root cannot be defined.
    pub fn define(&mut self, path: &PrimPath, prim_type: PrimType) -> Result<&mut Prim> {
        if path.is_root() {
            return Err(Error::InvalidPrimPath(
                "/".into(),
                "cannot define the pseudo-root",
            ));
        }
        if !self.prims.contains_key(path) {
            if let Some(parent) = path.parent() {
                if !parent.is_root() {
                    self.define(&parent, PrimType::Xform)?;
                    if let Some(pp) = self.prims.get_mut(&parent) {
                        if !pp.children.contains(path) {
                            pp.children.push(path.clone());
                        }
                    }
                }
            }
            self.prims
                .insert(path.clone(), Prim::new(path.clone(), prim_type));
        }
        self.prims
            .get_mut(path)
            .ok_or_else(|| Error::PrimNotFound(path.to_string()))
    }

    /// Insert a fully built prim; fails if the path is taken. Ancestors are
    /// created as needed; `prim.children` is reset (children are added by
    /// inserting them).
    pub fn insert(&mut self, mut prim: Prim) -> Result<()> {
        if self.prims.contains_key(&prim.path) {
            return Err(Error::PrimExists(prim.path.to_string()));
        }
        prim.children.clear();
        let path = prim.path.clone();
        let t = prim.prim_type;
        self.define(&path, t)?;
        if let Some(slot) = self.prims.get_mut(&path) {
            *slot = prim;
        }
        Ok(())
    }

    /// Remove a prim and its whole subtree. Returns the number removed.
    pub fn remove(&mut self, path: &PrimPath) -> usize {
        let Some(prim) = self.prims.shift_remove(path) else {
            return 0;
        };
        let mut n = 1;
        for c in &prim.children {
            n += self.remove(c);
        }
        if let Some(parent) = path.parent() {
            if let Some(pp) = self.prims.get_mut(&parent) {
                pp.children.retain(|c| c != path);
            }
        }
        n
    }

    /// Set the local transform of an existing prim.
    pub fn set_transform(&mut self, path: &PrimPath, t: Transform) -> Result<()> {
        let p = self
            .prims
            .get_mut(path)
            .ok_or_else(|| Error::PrimNotFound(path.to_string()))?;
        p.transform = t;
        Ok(())
    }

    /// World matrix of a prim: the product of every ancestor's local
    /// transform (Scope prims contribute identity). `None` if missing;
    /// the root yields identity.
    pub fn world_transform(&self, path: &PrimPath) -> Option<DMat4> {
        if path.is_root() {
            return Some(DMat4::IDENTITY);
        }
        let prim = self.prims.get(path)?;
        let parent = self.world_transform(&path.parent()?)?;
        let local = if prim.prim_type.is_xformable() {
            prim.transform.to_mat4()
        } else {
            DMat4::IDENTITY
        };
        Some(parent * local)
    }

    /// World-space transform of a prim as a TRS.
    pub fn world_trs(&self, path: &PrimPath) -> Option<Transform> {
        self.world_transform(path).map(|m| Transform::from_mat4(&m))
    }

    /// Union of every prim's world-space `extent`. `None` when no prim
    /// carries an extent.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut acc: Option<Bounds> = None;
        for prim in self.prims.values() {
            let Some((min, max)) = prim.extent() else {
                continue;
            };
            let Some(m) = self.world_transform(&prim.path) else {
                continue;
            };
            for i in 0..8 {
                let corner = DVec3::new(
                    if i & 1 == 0 { min[0] } else { max[0] },
                    if i & 2 == 0 { min[1] } else { max[1] },
                    if i & 4 == 0 { min[2] } else { max[2] },
                );
                let w = m.transform_point3(corner).to_array();
                acc = Some(match acc {
                    None => Bounds { min: w, max: w },
                    Some(b) => Bounds {
                        min: [b.min[0].min(w[0]), b.min[1].min(w[1]), b.min[2].min(w[2])],
                        max: [b.max[0].max(w[0]), b.max[1].max(w[1]), b.max[2].max(w[2])],
                    },
                });
            }
        }
        acc
    }

    /// Define a camera prim.
    pub fn add_camera(
        &mut self,
        path: &PrimPath,
        params: &CameraParams,
        transform: Transform,
    ) -> Result<PrimPath> {
        let p = self.define(path, PrimType::Camera)?;
        p.prim_type = PrimType::Camera;
        p.transform = transform;
        p.set_camera(params);
        Ok(path.clone())
    }

    /// Define a light prim.
    pub fn add_light(
        &mut self,
        path: &PrimPath,
        params: &LightParams,
        transform: Transform,
    ) -> Result<PrimPath> {
        let p = self.define(path, PrimType::Light)?;
        p.prim_type = PrimType::Light;
        p.transform = transform;
        p.set_light(params);
        Ok(path.clone())
    }

    /// Add a character stand-in under `/World/Characters/<Name>`: a
    /// `Character` prim `height_m` tall standing on `position` with a
    /// 0.6 × height × 0.6 m extent. The name is sanitized to an identifier.
    pub fn add_character(
        &mut self,
        name: &str,
        height_m: f64,
        position: [f64; 3],
    ) -> Result<PrimPath> {
        let path = PrimPath::new("/World/Characters")?.child(&sanitize_identifier(name))?;
        let p = self.define(&path, PrimType::Character)?;
        p.prim_type = PrimType::Character;
        p.transform = Transform::at(position[0], position[1], position[2]);
        p.set_extent([-0.3, 0.0, -0.3], [0.3, height_m, 0.3]);
        p.set_attr("cine:height", json!(height_m));
        p.set_attr("cine:displayName", json!(name));
        Ok(path)
    }

    // ---- USDA -----------------------------------------------------------

    /// Serialize the stage as USDA 1.0 text.
    pub fn to_usda(&self) -> String {
        let mut out = String::new();
        out.push_str("#usda 1.0\n(\n");
        let default_prim = self
            .default_prim
            .clone()
            .or_else(|| self.roots().first().map(|p| p.name().to_string()));
        if let Some(dp) = default_prim {
            out.push_str(&format!("    defaultPrim = {}\n", usda_string(&dp)));
        }
        out.push_str(&format!(
            "    metersPerUnit = {}\n",
            fmt_num(self.meters_per_unit)
        ));
        out.push_str(&format!("    upAxis = {}\n", usda_string(&self.up_axis)));
        out.push_str(")\n");
        for root in self.roots() {
            out.push('\n');
            self.write_prim(&mut out, root, 0);
        }
        out
    }

    fn write_prim(&self, out: &mut String, prim: &Prim, level: usize) {
        let ind = "    ".repeat(level);
        let ind2 = "    ".repeat(level + 1);
        out.push_str(&format!(
            "{ind}def {} {}",
            prim.usd_type_name(),
            usda_string(prim.name())
        ));
        if prim.prim_type == PrimType::Character {
            out.push_str(&format!(" (\n{ind2}kind = \"component\"\n{ind})"));
        }
        out.push_str(&format!("\n{ind}{{\n"));
        let mut wrote_any = false;
        if prim.prim_type.is_xformable() {
            let t = &prim.transform;
            let q = t.rotate;
            out.push_str(&format!(
                "{ind2}double3 xformOp:translate = {}\n",
                fmt_tuple(&t.translate)
            ));
            out.push_str(&format!(
                "{ind2}quatd xformOp:orient = {}\n",
                fmt_tuple(&[q[3], q[0], q[1], q[2]])
            ));
            out.push_str(&format!(
                "{ind2}double3 xformOp:scale = {}\n",
                fmt_tuple(&t.scale)
            ));
            out.push_str(&format!(
                "{ind2}uniform token[] xformOpOrder = [\"xformOp:translate\", \"xformOp:orient\", \"xformOp:scale\"]\n"
            ));
            wrote_any = true;
        }
        if prim.prim_type == PrimType::Character {
            out.push_str(&format!(
                "{ind2}custom token cine:primType = \"Character\"\n"
            ));
            wrote_any = true;
        }
        for (k, v) in &prim.attrs {
            if RESERVED_ATTRS.contains(&k.as_str()) {
                continue;
            }
            let (custom, ty) = usd_decl(k, v);
            out.push_str(&format!(
                "{ind2}{}{} {} = {}\n",
                if custom { "custom " } else { "" },
                ty,
                k,
                fmt_value(v, ty)
            ));
            wrote_any = true;
        }
        for (i, child) in self.children_of(&prim.path).into_iter().enumerate() {
            if wrote_any || i > 0 {
                out.push('\n');
            }
            self.write_prim(out, child, level + 1);
        }
        out.push_str(&format!("{ind}}}\n"));
    }

    /// Parse USDA text produced by [`Stage::to_usda`] (and simple
    /// hand-written files using the same subset: `def` prims with typed
    /// attributes, tuples, lists, strings, numbers and booleans).
    pub fn from_usda(text: &str) -> Result<Stage> {
        usda::parse(text)
    }
}

// ---------------------------------------------------------------------------
// USDA formatting helpers
// ---------------------------------------------------------------------------

/// Format a number so it parses back exactly.
pub(crate) fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let s = format!("{v}");
    if s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

fn fmt_tuple(v: &[f64]) -> String {
    let parts: Vec<String> = v.iter().map(|x| fmt_num(*x)).collect();
    format!("({})", parts.join(", "))
}

fn usda_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn is_num_array(a: &[Json]) -> bool {
    !a.is_empty() && a.iter().all(Json::is_number)
}

/// USD type declaration for an attribute: `(custom?, type name)`.
fn usd_decl(name: &str, value: &Json) -> (bool, &'static str) {
    let known: Option<&'static str> = match name {
        "focalLength"
        | "horizontalAperture"
        | "verticalAperture"
        | "horizontalApertureOffset"
        | "verticalApertureOffset"
        | "fStop"
        | "focusDistance" => Some("float"),
        "clippingRange" => Some("float2"),
        "extent" => Some("float3[]"),
        "purpose" | "visibility" | "projection" | "axis" => Some("token"),
        "inputs:intensity"
        | "inputs:exposure"
        | "inputs:colorTemperature"
        | "inputs:radius"
        | "inputs:width"
        | "inputs:height"
        | "inputs:angle"
        | "inputs:diffuse"
        | "inputs:specular"
        | "inputs:shaping:cone:angle"
        | "inputs:shaping:cone:softness" => Some("float"),
        "inputs:enableColorTemperature" | "inputs:normalize" => Some("bool"),
        "inputs:color" => Some("color3f"),
        "width" | "length" => Some("double"),
        _ => None,
    };
    if let Some(t) = known {
        return (false, t);
    }
    let ty = match value {
        Json::Bool(_) => "bool",
        Json::Number(_) => "double",
        Json::String(_) => "string",
        Json::Array(a) if is_num_array(a) => match a.len() {
            2 => "double2",
            3 => "double3",
            4 => "double4",
            _ => "double[]",
        },
        Json::Array(a)
            if a.iter().all(|x| {
                x.as_array()
                    .is_some_and(|i| i.len() == 3 && is_num_array(i))
            }) && !a.is_empty() =>
        {
            "double3[]"
        }
        Json::Array(a) if a.iter().all(Json::is_string) => "string[]",
        Json::Array(_) => "string",
        Json::Object(_) | Json::Null => "string",
    };
    (true, ty)
}

fn fmt_value(v: &Json, ty: &str) -> String {
    match v {
        Json::Bool(b) => b.to_string(),
        Json::Number(n) => fmt_num(n.as_f64().unwrap_or(0.0)),
        Json::String(s) => usda_string(s),
        Json::Array(a) => {
            let tuple_type = !ty.ends_with("[]") && is_num_array(a);
            let items: Vec<String> = a
                .iter()
                .map(|x| fmt_value(x, ty.trim_end_matches("[]")))
                .collect();
            if tuple_type {
                format!("({})", items.join(", "))
            } else if ty == "string" {
                usda_string(&v.to_string())
            } else {
                format!("[{}]", items.join(", "))
            }
        }
        Json::Object(_) | Json::Null => usda_string(&v.to_string()),
    }
}

// ---------------------------------------------------------------------------
// USDA parser (minimal)
// ---------------------------------------------------------------------------

mod usda {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    enum Tok {
        Ident(String),
        Num(f64),
        Str(String),
        LParen,
        RParen,
        LBracket,
        RBracket,
        LBrace,
        RBrace,
        Eq,
        Comma,
    }

    fn err<T>(line: usize, message: impl Into<String>) -> Result<T> {
        Err(Error::Usda {
            line,
            message: message.into(),
        })
    }

    fn tokenize(text: &str) -> Result<Vec<(Tok, usize)>> {
        let mut toks = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        let mut line = 1;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '\n' => {
                    line += 1;
                    i += 1;
                }
                c if c.is_whitespace() => i += 1,
                '#' => {
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                }
                '(' => {
                    toks.push((Tok::LParen, line));
                    i += 1;
                }
                ')' => {
                    toks.push((Tok::RParen, line));
                    i += 1;
                }
                '[' => {
                    toks.push((Tok::LBracket, line));
                    i += 1;
                }
                ']' => {
                    toks.push((Tok::RBracket, line));
                    i += 1;
                }
                '{' => {
                    toks.push((Tok::LBrace, line));
                    i += 1;
                }
                '}' => {
                    toks.push((Tok::RBrace, line));
                    i += 1;
                }
                '=' => {
                    toks.push((Tok::Eq, line));
                    i += 1;
                }
                ',' => {
                    toks.push((Tok::Comma, line));
                    i += 1;
                }
                '"' | '\'' => {
                    let quote = c;
                    i += 1;
                    let mut s = String::new();
                    loop {
                        if i >= chars.len() {
                            return err(line, "unterminated string");
                        }
                        let ch = chars[i];
                        if ch == quote {
                            i += 1;
                            break;
                        }
                        if ch == '\\' && i + 1 < chars.len() {
                            i += 1;
                            s.push(match chars[i] {
                                'n' => '\n',
                                't' => '\t',
                                other => other,
                            });
                        } else {
                            if ch == '\n' {
                                line += 1;
                            }
                            s.push(ch);
                        }
                        i += 1;
                    }
                    toks.push((Tok::Str(s), line));
                }
                '<' => {
                    // Path reference `</World/Foo>` — kept as a string.
                    i += 1;
                    let start = i;
                    while i < chars.len() && chars[i] != '>' {
                        i += 1;
                    }
                    if i >= chars.len() {
                        return err(line, "unterminated path reference");
                    }
                    let s: String = chars[start..i].iter().collect();
                    i += 1;
                    toks.push((Tok::Str(s), line));
                }
                c if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' => {
                    let start = i;
                    i += 1;
                    while i < chars.len()
                        && (chars[i].is_ascii_alphanumeric()
                            || chars[i] == '.'
                            || chars[i] == '-'
                            || chars[i] == '+')
                    {
                        // Stop at a sign not preceded by an exponent marker.
                        if (chars[i] == '-' || chars[i] == '+')
                            && !matches!(chars[i - 1], 'e' | 'E')
                        {
                            break;
                        }
                        i += 1;
                    }
                    let s: String = chars[start..i].iter().collect();
                    match s.parse::<f64>() {
                        Ok(v) => toks.push((Tok::Num(v), line)),
                        Err(_) => return err(line, format!("bad number `{s}`")),
                    }
                }
                c if c.is_ascii_alphabetic() || c == '_' => {
                    let start = i;
                    while i < chars.len()
                        && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '_' | ':' | '.'))
                    {
                        i += 1;
                    }
                    let s: String = chars[start..i].iter().collect();
                    toks.push((Tok::Ident(s), line));
                }
                other => return err(line, format!("unexpected character `{other}`")),
            }
        }
        Ok(toks)
    }

    struct Parser {
        toks: Vec<(Tok, usize)>,
        pos: usize,
    }

    impl Parser {
        fn peek(&self) -> Option<&Tok> {
            self.toks.get(self.pos).map(|(t, _)| t)
        }
        fn line(&self) -> usize {
            self.toks
                .get(self.pos)
                .or(self.toks.last())
                .map(|(_, l)| *l)
                .unwrap_or(0)
        }
        fn next(&mut self) -> Option<Tok> {
            let t = self.toks.get(self.pos).map(|(t, _)| t.clone());
            self.pos += 1;
            t
        }
        fn expect(&mut self, want: Tok) -> Result<()> {
            let line = self.line();
            match self.next() {
                Some(t) if t == want => Ok(()),
                other => err(line, format!("expected {want:?}, found {other:?}")),
            }
        }
        fn expect_ident(&mut self) -> Result<String> {
            let line = self.line();
            match self.next() {
                Some(Tok::Ident(s)) => Ok(s),
                other => err(line, format!("expected identifier, found {other:?}")),
            }
        }
        fn expect_str(&mut self) -> Result<String> {
            let line = self.line();
            match self.next() {
                Some(Tok::Str(s)) => Ok(s),
                other => err(line, format!("expected string, found {other:?}")),
            }
        }

        /// `( key = value ... )` metadata block → map.
        fn parse_metadata(&mut self) -> Result<IndexMap<String, Json>> {
            self.expect(Tok::LParen)?;
            let mut map = IndexMap::new();
            loop {
                match self.peek() {
                    Some(Tok::RParen) => {
                        self.pos += 1;
                        return Ok(map);
                    }
                    Some(Tok::Ident(_)) => {
                        let key = self.expect_ident()?;
                        if self.peek() == Some(&Tok::Eq) {
                            self.pos += 1;
                            let v = self.parse_value()?;
                            map.insert(key, v);
                        }
                    }
                    Some(Tok::Str(_)) => {
                        // Bare doc string.
                        let s = self.expect_str()?;
                        map.insert("doc".into(), Json::String(s));
                    }
                    Some(Tok::Comma) => self.pos += 1,
                    _ => return err(self.line(), "malformed metadata block"),
                }
            }
        }

        fn parse_value(&mut self) -> Result<Json> {
            let line = self.line();
            match self.next() {
                Some(Tok::Num(n)) => Ok(serde_json::Number::from_f64(n)
                    .map(Json::Number)
                    .unwrap_or(Json::Null)),
                Some(Tok::Str(s)) => Ok(Json::String(s)),
                Some(Tok::Ident(id)) => match id.as_str() {
                    "true" => Ok(Json::Bool(true)),
                    "false" => Ok(Json::Bool(false)),
                    "None" => Ok(Json::Null),
                    other => err(line, format!("unexpected identifier `{other}` in value")),
                },
                Some(Tok::LParen) => self.parse_seq(Tok::RParen),
                Some(Tok::LBracket) => self.parse_seq(Tok::RBracket),
                other => err(line, format!("unexpected {other:?} in value")),
            }
        }

        fn parse_seq(&mut self, close: Tok) -> Result<Json> {
            let mut items = Vec::new();
            loop {
                if self.peek() == Some(&close) {
                    self.pos += 1;
                    return Ok(Json::Array(items));
                }
                items.push(self.parse_value()?);
                match self.peek() {
                    Some(Tok::Comma) => self.pos += 1,
                    Some(t) if *t == close => {}
                    _ => return err(self.line(), "expected `,` or closing bracket"),
                }
            }
        }

        fn parse_prim(&mut self, stage: &mut Stage, parent: &PrimPath) -> Result<()> {
            let line = self.line();
            let kw = self.expect_ident()?;
            if kw != "def" && kw != "over" && kw != "class" {
                return err(line, format!("expected `def`, found `{kw}`"));
            }
            let usd_type = match self.peek() {
                Some(Tok::Ident(_)) => self.expect_ident()?,
                _ => "Xform".to_string(),
            };
            let name = self.expect_str()?;
            let path = parent
                .child(&sanitize_identifier(&name))
                .map_err(|e| Error::Usda {
                    line,
                    message: e.to_string(),
                })?;
            let mut meta = IndexMap::new();
            if self.peek() == Some(&Tok::LParen) {
                meta = self.parse_metadata()?;
            }
            self.expect(Tok::LBrace)?;
            let mut attrs: IndexMap<String, Json> = IndexMap::new();
            let mut children: Vec<(usize, ())> = Vec::new();
            // Create the prim first so children can attach to it.
            let prim_type = prim_type_from_usd(&usd_type);
            stage.define(&path, prim_type)?;
            loop {
                match self.peek() {
                    Some(Tok::RBrace) => {
                        self.pos += 1;
                        break;
                    }
                    Some(Tok::Ident(id)) if id == "def" || id == "over" || id == "class" => {
                        self.parse_prim(stage, &path)?;
                        children.push((self.pos, ()));
                    }
                    Some(Tok::Ident(_)) => {
                        // [modifiers] type[[]] name [= value]
                        let mut words: Vec<String> = Vec::new();
                        while let Some(Tok::Ident(_)) = self.peek() {
                            words.push(self.expect_ident()?);
                            if self.peek() == Some(&Tok::LBracket) {
                                // `type[]`
                                self.pos += 1;
                                self.expect(Tok::RBracket)?;
                                if let Some(last) = words.last_mut() {
                                    last.push_str("[]");
                                }
                            }
                        }
                        let Some(attr_name) = words.last().cloned() else {
                            return err(self.line(), "expected attribute name");
                        };
                        if words.first().map(String::as_str) == Some("rel") {
                            // Relationship: `rel name = <path>`; keep the target as string.
                            if self.peek() == Some(&Tok::Eq) {
                                self.pos += 1;
                                let v = self.parse_value()?;
                                attrs.insert(attr_name, v);
                            }
                            continue;
                        }
                        if self.peek() == Some(&Tok::Eq) {
                            self.pos += 1;
                            let v = self.parse_value()?;
                            attrs.insert(attr_name, v);
                        }
                        // Optional per-attribute metadata block.
                        if self.peek() == Some(&Tok::LParen) {
                            self.parse_metadata()?;
                        }
                    }
                    Some(Tok::Comma) => self.pos += 1,
                    _ => return err(self.line(), "unexpected token in prim body"),
                }
            }
            let prim = stage
                .prims
                .get_mut(&path)
                .ok_or_else(|| Error::PrimNotFound(path.to_string()))?;
            // Fold reserved attributes into typed fields.
            if let Some(t) = attrs.get("xformOp:translate").and_then(json_vec3) {
                prim.transform.translate = t;
            }
            if let Some(q) = attrs.get("xformOp:orient").and_then(json_vec4) {
                prim.transform.rotate = [q[1], q[2], q[3], q[0]];
            } else if let Some(e) = attrs.get("xformOp:rotateXYZ").and_then(json_vec3) {
                let q = DQuat::from_euler(
                    glam::EulerRot::XYZ,
                    e[0].to_radians(),
                    e[1].to_radians(),
                    e[2].to_radians(),
                );
                prim.transform.rotate = q.to_array();
            }
            if let Some(s) = attrs.get("xformOp:scale").and_then(json_vec3) {
                prim.transform.scale = s;
            }
            if let Some(t) = attrs
                .get("cine:primType")
                .and_then(Json::as_str)
                .and_then(prim_type_from_cine)
            {
                prim.prim_type = t;
            }
            for k in [
                "xformOp:translate",
                "xformOp:orient",
                "xformOp:scale",
                "xformOp:rotateXYZ",
                "xformOpOrder",
                "cine:primType",
            ] {
                attrs.shift_remove(k);
            }
            if !meta.is_empty() {
                if let Some(Json::String(k)) = meta.get("kind") {
                    if k == "component"
                        && prim.prim_type == PrimType::Xform
                        && attrs.contains_key("cine:height")
                    {
                        prim.prim_type = PrimType::Character;
                    }
                }
            }
            prim.attrs = attrs;
            Ok(())
        }
    }

    fn prim_type_from_usd(t: &str) -> PrimType {
        match t {
            "Camera" => PrimType::Camera,
            "Mesh" => PrimType::Mesh,
            "Scope" => PrimType::Scope,
            "Plane" => PrimType::Plane,
            "SphereLight" | "RectLight" | "DistantLight" | "DomeLight" | "DiskLight"
            | "CylinderLight" => PrimType::Light,
            _ => PrimType::Xform,
        }
    }

    fn prim_type_from_cine(t: &str) -> Option<PrimType> {
        Some(match t {
            "Xform" => PrimType::Xform,
            "Camera" => PrimType::Camera,
            "Light" => PrimType::Light,
            "Mesh" => PrimType::Mesh,
            "Scope" => PrimType::Scope,
            "Character" => PrimType::Character,
            "Plane" => PrimType::Plane,
            _ => return None,
        })
    }

    pub(super) fn parse(text: &str) -> Result<Stage> {
        let first = text.lines().next().unwrap_or("").trim();
        if !first.starts_with("#usda") {
            return err(1, "missing `#usda 1.0` header");
        }
        let toks = tokenize(text)?;
        let mut p = Parser { toks, pos: 0 };
        let mut stage = Stage::new();
        if p.peek() == Some(&Tok::LParen) {
            let meta = p.parse_metadata()?;
            if let Some(v) = meta.get("metersPerUnit").and_then(Json::as_f64) {
                stage.meters_per_unit = v;
            }
            if let Some(v) = meta.get("upAxis").and_then(Json::as_str) {
                stage.up_axis = v.to_string();
            }
            if let Some(v) = meta.get("defaultPrim").and_then(Json::as_str) {
                stage.default_prim = Some(v.to_string());
            }
        }
        let root = PrimPath::root();
        while p.peek().is_some() {
            p.parse_prim(&mut stage, &root)?;
        }
        Ok(stage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        let p = PrimPath::new("/World/Cameras/CamA").unwrap();
        assert_eq!(p.name(), "CamA");
        assert_eq!(p.parent().unwrap().as_str(), "/World/Cameras");
        assert_eq!(p.depth(), 3);
        assert_eq!(
            PrimPath::new("/World").unwrap().parent(),
            Some(PrimPath::root())
        );
        assert!(PrimPath::new("World").is_err());
        assert!(PrimPath::new("/World/").is_err());
        assert!(PrimPath::new("/World/Cam A").is_err());
        assert!(PrimPath::root().is_ancestor_of(&p));
        assert!(PrimPath::new("/World").unwrap().is_ancestor_of(&p));
        assert!(!PrimPath::new("/Wor").unwrap().is_ancestor_of(&p));
        assert_eq!(PrimPath::root().child("A").unwrap().as_str(), "/A");
        assert_eq!(
            sanitize_identifier("Dr. Ana Ruiz-Ortega"),
            "Dr_Ana_Ruiz_Ortega"
        );
        assert_eq!(sanitize_identifier("2nd Unit"), "_2nd_Unit");
        assert_eq!(sanitize_identifier("   "), "_");
    }

    #[test]
    fn define_creates_ancestors_and_remove_prunes() {
        let mut s = Stage::new();
        let cam = PrimPath::new("/World/Cameras/CamA").unwrap();
        s.define(&cam, PrimType::Camera).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(
            s.get(&PrimPath::new("/World").unwrap())
                .unwrap()
                .children
                .len(),
            1
        );
        assert_eq!(s.remove(&PrimPath::new("/World/Cameras").unwrap()), 2);
        assert_eq!(s.len(), 1);
        assert!(s
            .get(&PrimPath::new("/World").unwrap())
            .unwrap()
            .children
            .is_empty());
    }

    #[test]
    fn bounds_use_world_transforms() {
        let mut s = Stage::new();
        s.define(&PrimPath::new("/World").unwrap(), PrimType::Xform)
            .unwrap();
        s.set_transform(
            &PrimPath::new("/World").unwrap(),
            Transform::at(10.0, 0.0, 0.0),
        )
        .unwrap();
        s.add_character("Ana", 1.8, [1.0, 0.0, 0.0]).unwrap();
        let b = s.bounds().unwrap();
        assert!((b.min[0] - 10.7).abs() < 1e-9 && (b.max[0] - 11.3).abs() < 1e-9);
        assert!((b.max[1] - 1.8).abs() < 1e-9);
    }
}
