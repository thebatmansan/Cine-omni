// Previs viewport: physically based camera projection, Lambert + Blinn-Phong
// shading with physical lights, plus a grease-pencil ribbon pass.

struct CameraUniform {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // x = focus distance (m), y = f-stop, z = focal length (mm), w = sensor width (mm)
    optics: vec4<f32>,
    // x = exposure multiplier from ISO / shutter / aperture (EV-based), y = near, z = far
    exposure: vec4<f32>,
};

struct Light {
    position: vec4<f32>,   // w = 0 directional, 1 positional
    color: vec4<f32>,      // rgb linear * intensity (cd or lux), a = cone cos
    direction: vec4<f32>,  // spot direction, w = softness
};

struct Lights {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    items: array<Light, 16>,
};

struct ModelUniform {
    model: mat4x4<f32>,
    normal: mat4x4<f32>,
    base_color: vec4<f32>,
    material: vec4<f32>,  // x = roughness, y = metallic, z = emissive, w = id
};

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(0) @binding(1) var<uniform> lights: Lights;
@group(1) @binding(0) var<uniform> model: ModelUniform;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) view_depth: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let wp = model.model * vec4<f32>(in.position, 1.0);
    out.world_pos = wp.xyz;
    out.world_normal = normalize((model.normal * vec4<f32>(in.normal, 0.0)).xyz);
    out.uv = in.uv;
    out.clip = camera.view_proj * wp;
    out.view_depth = -(camera.view * wp).z;
    return out;
}

struct FsOut {
    @location(0) color: vec4<f32>,
    @location(1) depth: vec4<f32>,   // linear view depth (m) in .r, id in .g
    @location(2) normal: vec4<f32>,
};

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let n = normalize(in.world_normal);
    let v = normalize(camera.camera_pos.xyz - in.world_pos);
    var radiance = vec3<f32>(0.0);
    for (var i = 0u; i < lights.count; i = i + 1u) {
        let L = lights.items[i];
        var l: vec3<f32>;
        var atten = 1.0;
        if (L.position.w < 0.5) {
            l = normalize(-L.direction.xyz);
        } else {
            let d = L.position.xyz - in.world_pos;
            let dist2 = max(dot(d, d), 1e-4);
            l = d / sqrt(dist2);
            atten = 1.0 / dist2;                         // inverse square (candela -> lux)
            if (L.color.a > -1.0) {                       // spot cone
                let cos_theta = dot(-l, normalize(L.direction.xyz));
                let soft = max(L.direction.w, 1e-3);
                atten = atten * smoothstep(L.color.a, L.color.a + soft, cos_theta);
            }
        }
        let ndl = max(dot(n, l), 0.0);
        let h = normalize(l + v);
        let rough = max(model.material.x, 0.05);
        let spec = pow(max(dot(n, h), 0.0), 2.0 / (rough * rough) - 2.0) * (1.0 - rough);
        radiance = radiance + L.color.rgb * atten * (model.base_color.rgb * ndl + vec3<f32>(spec * 0.04));
    }
    radiance = radiance * camera.exposure.x + model.base_color.rgb * model.material.z;
    var out: FsOut;
    out.color = vec4<f32>(radiance, 1.0);
    out.depth = vec4<f32>(in.view_depth, model.material.w, 0.0, 1.0);
    out.normal = vec4<f32>(n * 0.5 + 0.5, 1.0);
    return out;
}

// ---- Grease pencil ribbons ------------------------------------------------

struct RibbonVsIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct RibbonVsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_ribbon(in: RibbonVsIn) -> RibbonVsOut {
    var out: RibbonVsOut;
    out.clip = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_ribbon(in: RibbonVsOut) -> @location(0) vec4<f32> {
    return in.color;
}
