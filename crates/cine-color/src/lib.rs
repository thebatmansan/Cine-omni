//! # cine-color — an ACES-shaped colour pipeline for CINE-OMNI
//!
//! CPU reference implementation of the colour pipeline: colorimetry and
//! transfer functions ([`space`], [`transfer`]), optimized op chains
//! ([`processor`]), ACES matrices / encodings / output transforms ([`aces`]),
//! an OpenColorIO-*shaped* config reader ([`ocio`]), `.cube` LUTs ([`lut`]),
//! grading primaries, curves, secondaries and CDL export ([`grade`], [`curve`],
//! [`mask`]), scopes ([`scopes`]) and the graph integration ([`ucg`]).
//!
//! ## What this is not — read before relying on it
//!
//! This crate is **not ACES-compatible and not OpenColorIO-compatible**, and
//! must not be described as either. Both claims were tested against the real
//! implementations and both failed:
//!
//! * **It cannot load any released OpenColorIO ACES config.** All three that
//!   OCIO ships (ACES 2.0 CG, ACES 2.0 Studio, ACES 1.3 CG) fail on the first
//!   display block, because [`ocio`] does not implement `shared_views` or the
//!   `!<Views>` tag — core OCIO v2 features present in every ACES config since
//!   2021. What it reads is a subset of the config *syntax*, not OCIO.
//! * **Its ACES output transform is exact on neutrals and badly wrong on
//!   saturated colour.** Measured against OpenColorIO 2.6.0dev as an oracle,
//!   ACEScg to ACES 2.0 SDR Rec.709: 18% grey matches to six decimal places,
//!   while pure green differs by 0.30 — 30% of full scale. The tonescale is
//!   right; the chroma handling and gamut compression are absent.
//!
//! Treat it as a working CPU colour engine whose neutral axis is trustworthy,
//! and whose saturated-colour path is not yet a display-referred answer anyone
//! should grade against. See `docs/blueprint/research/13-color-management.md`
//! for the measurements and for what closing the gap requires.
//!
//! ```
//! use cine_color::prelude::*;
//!
//! // Camera log → ACEScg → sRGB display through the built-in ACES config.
//! let config = Config::aces_default();
//! let to_working = config.processor("ARRI LogC4", "ACEScg").unwrap();
//! let gray = to_working.apply_rgb(&[Transfer::LogC4.encode(0.18) as f32; 3]);
//! assert!((gray[0] - 0.18).abs() < 1e-3);
//!
//! let view = config.display_processor("ACEScg", "sRGB", "ACES 1.0 - SDR Video").unwrap();
//! let code = view.apply_rgb(&gray);
//! assert!(code[0] > 0.3 && code[0] < 0.5);
//!
//! // A grade: exposure + a green qualifier, exported as ASC CDL.
//! let mut stack = GradeStack::primary(vec![GradeOp::Exposure { stops: 0.5 }]);
//! stack.layers.push(GradeLayer { qualifier: Some(HslQualifier::hue(120.0, 60.0, 10.0)), ops: vec![GradeOp::Saturation { amount: 1.3 }], ..Default::default() });
//! let cdl = stack.to_cdl();
//! assert!((cdl.slope[0] - 2f64.sqrt()).abs() < 1e-6);
//! ```

pub mod aces;
pub mod curve;
pub mod error;
pub mod grade;
pub mod lut;
pub mod mask;
pub mod ocio;
pub mod processor;
pub mod scopes;
pub mod space;
pub mod transfer;
pub mod ucg;
pub mod yaml;

pub use aces::{idt, rrt_odt_fit_srgb, Camera, Display, HueMode, OutputTransform, TonescaleParams};
pub use curve::{Curve, Curves};
pub use error::{Error, Result};
pub use grade::{parse_cdl_xml, write_cdl_xml, GradeLayer, GradeOp, GradeStack, Wheels};
pub use lut::{parse_cube, Interpolation, Lut, Lut1D, Lut3D};
pub use mask::{CombineMode, HslQualifier, Mask};
pub use ocio::{ColorSpaceDef, Config, TransformDef, View};
pub use processor::{Direction, Op, Processor, Transform};
pub use scopes::{
    clipping_stats, hdr_waveform_nits, histogram, rgb_parade, vectorscope, waveform, Histogram,
    ImageView, Vectorscope, Waveform,
};
pub use space::{ColorSpace, Mat3, Primaries, PrimariesId};
pub use transfer::Transfer;
pub use ucg_schema::{Cdl, ColorMeta};

/// Everything most callers need.
pub mod prelude {
    pub use crate::aces::{idt, rrt_odt_fit_srgb, Camera, Display, HueMode, OutputTransform};
    pub use crate::curve::{Curve, Curves};
    pub use crate::error::{Error, Result};
    pub use crate::grade::{parse_cdl_xml, write_cdl_xml, GradeLayer, GradeOp, GradeStack, Wheels};
    pub use crate::lut::{parse_cube, Interpolation, Lut, Lut1D, Lut3D};
    pub use crate::mask::{CombineMode, HslQualifier, Mask};
    pub use crate::ocio::Config;
    pub use crate::processor::{Op, Processor, Transform};
    pub use crate::scopes::{
        clipping_stats, hdr_waveform_nits, histogram, rgb_parade, vectorscope, waveform, ImageView,
    };
    pub use crate::space::{ColorSpace, Mat3, Primaries, PrimariesId};
    pub use crate::transfer::Transfer;
    pub use crate::ucg::{
        ensure_grade_for_clip, grades_for, install_config, load_grade, processor_for_clip,
        propagate_color_meta, register_lut, store_grade,
    };
    pub use ucg_schema::{Cdl, ColorMeta};
}
