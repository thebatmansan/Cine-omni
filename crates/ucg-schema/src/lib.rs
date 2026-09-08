//! # ucg-schema — the shared vocabulary of CINE-OMNI
//!
//! Node kinds, attribute keys, and typed components that cross module
//! boundaries. Every module reads and writes the graph through these names,
//! which is what makes "one UUID, every module" work.
//!
//! Conventions
//! * Node kinds are `module.type` strings (see [`kinds`]).
//! * Attribute keys are short snake_case strings (see [`attrs`]).
//! * Typed components implement [`Component`] and live in one attribute each,
//!   serialized as a `Value` tree. Modules may add private attributes freely.
//! * Derived entities use `EntityId::derived(source, kinds::X)`.

pub mod component;
pub mod components;
pub mod kinds;

pub use component::Component;
pub use components::*;
pub use ucg_core::{Color, EntityId, RationalTime, TimeRange, Value};

/// Attribute keys shared across modules.
pub mod attrs {
    pub const NAME: &str = "name";
    pub const ORDER: &str = "order";
    pub const TEXT: &str = "text";
    pub const NOTES: &str = "notes";
    pub const TAGS: &str = "tags";
    pub const SOURCE_KEY: &str = "source_key";
    pub const STALE: &str = "stale";
    pub const NEEDS_CONFORM: &str = "needs_conform";
    pub const DIRTY_REASON: &str = "dirty_reason";

    // script
    pub const HEADING: &str = "heading";
    pub const SCENE_NUMBER: &str = "scene_number";
    pub const CHARACTER: &str = "character";
    pub const EXTENSION: &str = "extension"; // (V.O.), (O.S.)
    pub const DUAL: &str = "dual";
    pub const CHARACTERS: &str = "characters";
    pub const LOCATIONS: &str = "locations";
    pub const PROPS: &str = "props";
    pub const PAGE_EIGHTHS: &str = "page_eighths";
    pub const EST_DURATION_S: &str = "est_duration_s";
    pub const WORDS: &str = "words";
    pub const INT_EXT: &str = "int_ext";
    pub const LOCATION: &str = "location";
    pub const TIME_OF_DAY: &str = "time_of_day";

    // spatial
    pub const TRANSFORM: &str = "transform";
    pub const BOUNDS: &str = "bounds";
    pub const CAMERA: &str = "camera";
    pub const LIGHT: &str = "light";
    pub const PRIM_PATH: &str = "prim_path";
    pub const PRIM_TYPE: &str = "prim_type";

    // time / editorial
    pub const RATE: &str = "rate";
    pub const SOURCE_RANGE: &str = "source_range";
    pub const RANGE: &str = "range";
    pub const MEDIA: &str = "media";
    pub const MEDIA_PATH: &str = "media_path";
    pub const AVAILABLE_RANGE: &str = "available_range";
    pub const TRACK_KIND: &str = "track_kind";
    pub const ALIGNMENT: &str = "alignment";

    // color
    pub const COLOR_META: &str = "color_meta";
    pub const COLORSPACE: &str = "colorspace";
    pub const LUT: &str = "lut";
    pub const CDL: &str = "cdl";

    // logistics
    pub const SHOT: &str = "shot";
    pub const DAY: &str = "day";
    pub const METADATA: &str = "metadata";
    pub const SCHEDULE: &str = "schedule";
}
