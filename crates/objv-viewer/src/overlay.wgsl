// Overlay shader for measurement lines, section profiles and dip discs.
// Plain colored lines, transformed by the same MVP as the mesh. Drawn with the
// depth test disabled so annotations stay visible through the surface.

struct Uniforms {
    mvp: mat4x4<f32>,
    light_dir: vec4<f32>,
    z_min: f32,
    z_max: f32,
    mode: u32,
    _pad: f32,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>, @location(1) color: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip = u.mvp * vec4<f32>(pos, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
