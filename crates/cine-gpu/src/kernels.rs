//! Embedded WGSL kernels and their metadata.

/// Shared helper prelude prepended to every compute kernel.
pub const COMMON_WGSL: &str = include_str!("shaders/common.wgsl");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KernelId {
    Merge,
    Cdl,
    Lut3D,
    Scopes,
    Blur,
    Keyer,
    Dof,
    /// Planar YUV to RGB, for presenting decoded camera media.
    YuvToRgb,
    /// Vertex/fragment viewport shader (render pipeline, not compute).
    Previs,
}

#[derive(Clone, Copy, Debug)]
pub enum KernelStage {
    Compute {
        entry: &'static str,
        workgroup: [u32; 3],
    },
    Render {
        vertex: &'static str,
        fragment: &'static str,
    },
}

/// How a kernel binding is declared in WGSL. The pipeline layout must match
/// the shader exactly, so each binding states its access here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    /// `var<uniform>` parameter block.
    Uniform,
    /// `var<storage, read>` input buffer.
    Read,
    /// `var<storage, read_write>` output buffer.
    Write,
}

#[derive(Clone, Copy, Debug)]
pub struct Kernel {
    pub id: KernelId,
    pub name: &'static str,
    pub source: &'static str,
    pub stage: KernelStage,
    /// Bindings of group 0, in binding order.
    pub bindings: &'static [Binding],
}

impl Kernel {
    /// Full WGSL module text (prelude + kernel).
    pub fn module_source(&self) -> String {
        format!("{}\n{}", COMMON_WGSL, self.source)
    }

    /// Number of bindings in group 0.
    pub fn binding_count(&self) -> u32 {
        self.bindings.len() as u32
    }

    /// Dispatch size for a 2D image at the kernel's workgroup size.
    pub fn dispatch_2d(&self, width: u32, height: u32) -> [u32; 3] {
        match self.stage {
            KernelStage::Compute { workgroup, .. } => [
                width.div_ceil(workgroup[0]),
                height.div_ceil(workgroup[1]),
                1,
            ],
            KernelStage::Render { .. } => [0, 0, 0],
        }
    }
}

const WG: [u32; 3] = [16, 16, 1];

use Binding::{Read, Uniform, Write};

pub const KERNELS: &[Kernel] = &[
    Kernel {
        id: KernelId::YuvToRgb,
        name: "yuv_to_rgb",
        source: include_str!("shaders/yuv_to_rgb.wgsl"),
        stage: KernelStage::Compute {
            entry: "main",
            workgroup: WG,
        },
        bindings: &[Uniform, Read, Read, Read, Write],
    },
    Kernel {
        id: KernelId::Merge,
        name: "merge",
        source: include_str!("shaders/merge.wgsl"),
        stage: KernelStage::Compute {
            entry: "main",
            workgroup: WG,
        },
        bindings: &[Uniform, Read, Read, Write],
    },
    Kernel {
        id: KernelId::Cdl,
        name: "cdl",
        source: include_str!("shaders/cdl.wgsl"),
        stage: KernelStage::Compute {
            entry: "main",
            workgroup: WG,
        },
        bindings: &[Uniform, Read, Write],
    },
    Kernel {
        id: KernelId::Lut3D,
        name: "lut3d",
        source: include_str!("shaders/lut3d.wgsl"),
        stage: KernelStage::Compute {
            entry: "main",
            workgroup: WG,
        },
        bindings: &[Uniform, Read, Read, Write],
    },
    Kernel {
        id: KernelId::Scopes,
        name: "scopes",
        source: include_str!("shaders/scopes.wgsl"),
        stage: KernelStage::Compute {
            entry: "main",
            workgroup: WG,
        },
        bindings: &[Uniform, Read, Write, Write, Write],
    },
    Kernel {
        id: KernelId::Blur,
        name: "blur",
        source: include_str!("shaders/blur.wgsl"),
        stage: KernelStage::Compute {
            entry: "main",
            workgroup: WG,
        },
        bindings: &[Uniform, Read, Write],
    },
    Kernel {
        id: KernelId::Keyer,
        name: "keyer",
        source: include_str!("shaders/keyer.wgsl"),
        stage: KernelStage::Compute {
            entry: "main",
            workgroup: WG,
        },
        bindings: &[Uniform, Read, Write],
    },
    Kernel {
        id: KernelId::Dof,
        name: "dof",
        source: include_str!("shaders/dof.wgsl"),
        stage: KernelStage::Compute {
            entry: "main",
            workgroup: WG,
        },
        bindings: &[Uniform, Read, Read, Write],
    },
    Kernel {
        id: KernelId::Previs,
        name: "previs",
        source: include_str!("shaders/previs.wgsl"),
        stage: KernelStage::Render {
            vertex: "vs_main",
            fragment: "fs_main",
        },
        bindings: &[Uniform, Uniform],
    },
];

pub fn kernel(id: KernelId) -> &'static Kernel {
    KERNELS
        .iter()
        .find(|k| k.id == id)
        .expect("every KernelId is registered")
}

/// Merge operations; discriminants match the `op` switch in `merge.wgsl`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum MergeOp {
    Over = 0,
    Under = 1,
    Plus = 2,
    Minus = 3,
    Multiply = 4,
    Screen = 5,
    Difference = 6,
    Max = 7,
    Min = 8,
    Mask = 9,
    Stencil = 10,
    Xor = 11,
    In = 12,
    Out = 13,
    Atop = 14,
}

impl MergeOp {
    pub const ALL: [MergeOp; 15] = [
        MergeOp::Over,
        MergeOp::Under,
        MergeOp::Plus,
        MergeOp::Minus,
        MergeOp::Multiply,
        MergeOp::Screen,
        MergeOp::Difference,
        MergeOp::Max,
        MergeOp::Min,
        MergeOp::Mask,
        MergeOp::Stencil,
        MergeOp::Xor,
        MergeOp::In,
        MergeOp::Out,
        MergeOp::Atop,
    ];
}

/// Uniform layouts mirrored from the WGSL structs (std140-compatible sizes).
pub mod uniforms {
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct MergeParams {
        pub width: u32,
        pub height: u32,
        pub op: u32,
        pub mix: f32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct CdlParams {
        pub slope: [f32; 4],
        pub offset: [f32; 4],
        pub power: [f32; 4],
        pub saturation: f32,
        pub width: u32,
        pub height: u32,
        pub _pad: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct LutParams {
        pub size: u32,
        pub width: u32,
        pub height: u32,
        pub _pad: u32,
        pub domain_min: [f32; 4],
        pub domain_max: [f32; 4],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct BlurParams {
        pub width: u32,
        pub height: u32,
        pub radius: u32,
        pub horizontal: u32,
        pub sigma: f32,
        pub _p0: f32,
        pub _p1: f32,
        pub _p2: f32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct KeyParams {
        pub key_color: [f32; 4],
        pub width: u32,
        pub height: u32,
        pub tolerance: f32,
        pub softness: f32,
        pub spill: f32,
        pub _p0: f32,
        pub _p1: f32,
        pub _p2: f32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug)]
    pub struct DofParams {
        pub width: u32,
        pub height: u32,
        pub max_radius: u32,
        pub _pad: u32,
        pub optics: [f32; 4],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default)]
    pub struct ScopeParams {
        pub width: u32,
        pub height: u32,
        pub hdr_pq: u32,
        pub _pad: u32,
    }

    /// View any `#[repr(C)]` plain-data uniform as bytes for upload.
    ///
    /// # Safety contract
    /// Only used with the POD structs above (no padding-dependent reads).
    pub fn as_bytes<T: Copy>(t: &T) -> &[u8] {
        // SAFETY: T is a repr(C) POD struct composed of u32/f32 fields.
        unsafe {
            std::slice::from_raw_parts((t as *const T) as *const u8, std::mem::size_of::<T>())
        }
    }
}
