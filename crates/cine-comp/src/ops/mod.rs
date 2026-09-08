//! CPU reference implementations of every node operation. These are pure
//! functions over [`crate::image::Image`] / [`crate::image::DeepImage`]; the
//! evaluator ([`crate::eval`]) maps node parameters onto them.

pub mod color;
pub mod deep;
pub mod defocus;
pub mod filter;
pub mod keyer;
pub mod merge;
pub mod procedural;
pub mod roto;
pub mod transform;

pub use color::{
    clamp, color_correct, copy_channels, expression, invert, premult, shuffle, unpremult,
    ColorCorrect,
};
pub use deep::{deep_from_image, deep_holdout, deep_merge, deep_to_image};
pub use defocus::{z_defocus, ZDefocus};
pub use filter::{
    blur, blur_masked, dilate_alpha, edge_blur, erode_alpha, gaussian_kernel, sharpen,
};
pub use keyer::{chroma_key, despill, difference_key, luma_key, screen_key, ChromaKey};
pub use merge::{merge, merge_pixel};
pub use procedural::{generate, Perlin, Procedural};
pub use roto::rasterize;
pub use transform::{crop, reformat, transform, Transform2D};
