//! # cine-gpu — GPU pipeline for CINE-OMNI
//!
//! * [`kernels`] — the WGSL kernel registry (compositing, color, scopes,
//!   depth of field, previs viewport). Kernels are embedded at compile time and
//!   validated by `naga` in tests, so a broken shader fails CI without a GPU.
//! * [`frame_graph`] — backend-agnostic render/compute graph with automatic
//!   pass ordering and transient-resource aliasing.
//! * [`cpu`] — CPU mirrors of the pixel kernels, used as the reference the GPU
//!   path is validated against.
//! * [`device`] (feature `wgpu`) — the real wgpu backend: device creation,
//!   pipeline compilation, buffer kernels, readback.
//!
//! The GPU is treated as an accelerator for the CPU reference implementations
//! in `cine-comp` and `cine-color`; results must match within tolerance.

pub mod cpu;
pub mod frame_graph;
pub mod kernels;

#[cfg(feature = "wgpu")]
pub mod device;

pub use frame_graph::{FrameGraph, Pass, ResourceDesc, ResourceId};
pub use kernels::{Kernel, KernelId, MergeOp, KERNELS};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GpuError {
    #[error("no compatible GPU adapter found")]
    NoAdapter,
    #[error("device error: {0}")]
    Device(String),
    #[error("shader `{name}` failed to compile: {message}")]
    Shader { name: String, message: String },
    #[error("frame graph: {0}")]
    Graph(String),
    #[error("buffer size mismatch: expected {expected} floats, got {actual}")]
    Size { expected: usize, actual: usize },
}

pub type Result<T> = std::result::Result<T, GpuError>;
