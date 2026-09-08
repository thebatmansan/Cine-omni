//! Dynamically typed attribute values.
//!
//! The UCG stores attributes as [`Value`]s so modules can attach data without
//! central schema changes (the same idea as USD attributes). Strongly typed
//! component structs in `ucg-schema` convert to and from `Value` via serde.

use crate::id::EntityId;
use crate::time::{RationalTime, TimeRange};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Linear RGBA color in the project working space (scene-linear by default).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Color { r, g, b, a }
    }
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Color { r, g, b, a: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Map(IndexMap<String, Value>),
    /// Reference to another entity by UUID. Cross-module links live here or in edges.
    Ref(EntityId),
    Vec2([f64; 2]),
    Vec3([f64; 3]),
    Vec4([f64; 4]),
    Quat([f64; 4]),
    Mat4([f64; 16]),
    Color(Color),
    Time(RationalTime),
    Range(TimeRange),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "str",
            Value::Bytes(_) => "bytes",
            Value::List(_) => "list",
            Value::Map(_) => "map",
            Value::Ref(_) => "ref",
            Value::Vec2(_) => "vec2",
            Value::Vec3(_) => "vec3",
            Value::Vec4(_) => "vec4",
            Value::Quat(_) => "quat",
            Value::Mat4(_) => "mat4",
            Value::Color(_) => "color",
            Value::Time(_) => "time",
            Value::Range(_) => "range",
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        }
    }
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Value]> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }
    pub fn as_map(&self) -> Option<&IndexMap<String, Value>> {
        match self {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_ref(&self) -> Option<EntityId> {
        match self {
            Value::Ref(r) => Some(*r),
            _ => None,
        }
    }
    pub fn as_vec3(&self) -> Option<[f64; 3]> {
        match self {
            Value::Vec3(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_time(&self) -> Option<RationalTime> {
        match self {
            Value::Time(t) => Some(*t),
            _ => None,
        }
    }
    pub fn as_range(&self) -> Option<TimeRange> {
        match self {
            Value::Range(r) => Some(*r),
            _ => None,
        }
    }
    pub fn as_color(&self) -> Option<Color> {
        match self {
            Value::Color(c) => Some(*c),
            _ => None,
        }
    }

    /// Convenience: list of entity references.
    pub fn as_ref_list(&self) -> Vec<EntityId> {
        self.as_list()
            .map(|l| l.iter().filter_map(|v| v.as_ref()).collect())
            .unwrap_or_default()
    }

    /// Convert any serde-serializable struct into a `Value` tree.
    pub fn from_serde<T: Serialize>(t: &T) -> crate::Result<Value> {
        let json = serde_json::to_value(t)?;
        Ok(Value::from_json(json))
    }

    /// Convert a `Value` tree back into a typed struct.
    pub fn to_serde<T: for<'de> Deserialize<'de>>(&self) -> crate::Result<T> {
        let json = self.to_json();
        Ok(serde_json::from_value(json)?)
    }

    /// Lossy-but-practical mapping from JSON. Tagged structs (vec3 etc.) survive
    /// because `from_serde` produces JSON with the `{"t":..,"v":..}` shape only
    /// for `Value` fields themselves; plain numbers/strings map directly.
    pub fn from_json(j: serde_json::Value) -> Value {
        use serde_json::Value as J;
        match j {
            J::Null => Value::Null,
            J::Bool(b) => Value::Bool(b),
            J::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Value::Int(i)
                } else {
                    Value::Float(n.as_f64().unwrap_or(0.0))
                }
            }
            J::String(s) => Value::Str(s),
            J::Array(a) => Value::List(a.into_iter().map(Value::from_json).collect()),
            J::Object(o) => {
                // Recognize our own tagged encoding so round trips are exact.
                if o.len() == 2 && o.contains_key("t") && o.contains_key("v") {
                    if let Ok(v) = serde_json::from_value::<Value>(J::Object(o.clone())) {
                        return v;
                    }
                }
                Value::Map(
                    o.into_iter()
                        .map(|(k, v)| (k, Value::from_json(v)))
                        .collect(),
                )
            }
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match self {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Int(i) => J::from(*i),
            Value::Float(f) => J::from(*f),
            Value::Str(s) => J::String(s.clone()),
            Value::List(l) => J::Array(l.iter().map(Value::to_json).collect()),
            Value::Map(m) => J::Object(m.iter().map(|(k, v)| (k.clone(), v.to_json())).collect()),
            // Everything else keeps the tagged form so it deserializes back exactly.
            other => serde_json::to_value(other).unwrap_or(J::Null),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Ref(r) => write!(f, "@{}", r.short()),
            Value::Time(t) => write!(f, "{t}"),
            Value::Range(r) => write!(f, "{r}"),
            other => write!(f, "{}", other.to_json()),
        }
    }
}

// --- ergonomic conversions -------------------------------------------------

macro_rules! from_impl {
    ($t:ty, $variant:ident, $conv:expr) => {
        impl From<$t> for Value {
            fn from(v: $t) -> Self {
                Value::$variant($conv(v))
            }
        }
    };
}
from_impl!(bool, Bool, |v| v);
from_impl!(i64, Int, |v| v);
from_impl!(i32, Int, |v: i32| v as i64);
from_impl!(u32, Int, |v: u32| v as i64);
from_impl!(usize, Int, |v: usize| v as i64);
from_impl!(f64, Float, |v| v);
from_impl!(f32, Float, |v: f32| v as f64);
from_impl!(String, Str, |v| v);
from_impl!(&str, Str, |v: &str| v.to_string());
from_impl!(EntityId, Ref, |v| v);
from_impl!([f64; 3], Vec3, |v| v);
from_impl!([f64; 2], Vec2, |v| v);
from_impl!(Color, Color, |v| v);
from_impl!(RationalTime, Time, |v| v);
from_impl!(TimeRange, Range, |v| v);
from_impl!(Vec<u8>, Bytes, |v| v);

impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::List(v)
    }
}
impl From<Vec<EntityId>> for Value {
    fn from(v: Vec<EntityId>) -> Self {
        Value::List(v.into_iter().map(Value::Ref).collect())
    }
}
impl From<Vec<String>> for Value {
    fn from(v: Vec<String>) -> Self {
        Value::List(v.into_iter().map(Value::Str).collect())
    }
}
impl From<glam::DVec3> for Value {
    fn from(v: glam::DVec3) -> Self {
        Value::Vec3(v.to_array())
    }
}
impl From<glam::DMat4> for Value {
    fn from(m: glam::DMat4) -> Self {
        Value::Mat4(m.to_cols_array())
    }
}
impl From<glam::DQuat> for Value {
    fn from(q: glam::DQuat) -> Self {
        Value::Quat(q.to_array())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip_keeps_tagged_types() {
        let v = Value::Map(IndexMap::from([
            ("pos".to_string(), Value::Vec3([1.0, 2.0, 3.0])),
            ("t".to_string(), Value::Time(RationalTime::new(24.0, 24.0))),
            ("n".to_string(), Value::Int(3)),
            ("name".to_string(), Value::Str("x".into())),
        ]));
        let j = v.to_json();
        let back = Value::from_json(j);
        assert_eq!(v, back);
    }

    #[test]
    fn serde_struct_round_trip() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct S {
            a: i32,
            b: String,
            c: Vec<f64>,
        }
        let s = S {
            a: 1,
            b: "hi".into(),
            c: vec![1.5, 2.5],
        };
        let v = Value::from_serde(&s).unwrap();
        let back: S = v.to_serde().unwrap();
        assert_eq!(s, back);
    }
}
