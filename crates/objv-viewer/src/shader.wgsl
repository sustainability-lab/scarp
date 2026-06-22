// OBJV viewer shader.
//
// Normals are NOT stored in the mesh; the fragment stage reconstructs a
// per-face normal from the screen-space derivatives of the interpolated world
// position. That gives a crisp faceted "hillshade" — the look geologists read
// structure from — and lets the file omit the entire normal buffer.

struct Uniforms {
    mvp: mat4x4<f32>,
    light_dir: vec4<f32>, // xyz: world-space direction toward the light
    z_min: f32,
    z_max: f32,
    mode: u32,            // 0 shaded, 1 elevation, 2 slope, 3 aspect
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip = u.mvp * vec4<f32>(pos, 1.0);
    out.world = pos;
    return out;
}

// --- colormaps -------------------------------------------------------------

// A compact terrain ramp: deep green -> tan -> brown -> grey/white.
fn terrain(t: f32) -> vec3<f32> {
    let s = clamp(t, 0.0, 1.0);
    let c0 = vec3<f32>(0.18, 0.36, 0.22); // low: vegetated
    let c1 = vec3<f32>(0.55, 0.52, 0.30); // tan
    let c2 = vec3<f32>(0.52, 0.40, 0.28); // brown rock
    let c3 = vec3<f32>(0.85, 0.84, 0.82); // high: bare/light
    if (s < 0.33) {
        return mix(c0, c1, s / 0.33);
    } else if (s < 0.66) {
        return mix(c1, c2, (s - 0.33) / 0.33);
    } else {
        return mix(c2, c3, (s - 0.66) / 0.34);
    }
}

// HSV->RGB for the aspect (compass-direction) wheel.
fn hsv(h: f32, s: f32, v: f32) -> vec3<f32> {
    let c = v * s;
    let x = c * (1.0 - abs((h / 60.0) % 2.0 - 1.0));
    let m = v - c;
    var rgb = vec3<f32>(0.0);
    if (h < 60.0) { rgb = vec3<f32>(c, x, 0.0); }
    else if (h < 120.0) { rgb = vec3<f32>(x, c, 0.0); }
    else if (h < 180.0) { rgb = vec3<f32>(0.0, c, x); }
    else if (h < 240.0) { rgb = vec3<f32>(0.0, x, c); }
    else if (h < 300.0) { rgb = vec3<f32>(x, 0.0, c); }
    else { rgb = vec3<f32>(c, 0.0, x); }
    return rgb + vec3<f32>(m, m, m);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Face normal from derivatives of world position. Z is up (UTM elevation).
    let n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    let nf = select(n, -n, n.z < 0.0); // orient upward for consistent shading

    // Hillshade: one key light + hemispheric ambient so cavities stay readable.
    let l = normalize(u.light_dir.xyz);
    let diff = max(dot(nf, l), 0.0);
    let ambient = 0.35 + 0.25 * (nf.z * 0.5 + 0.5);
    let shade = clamp(ambient + 0.75 * diff, 0.0, 1.2);

    var albedo: vec3<f32>;
    switch (u.mode) {
        case 1u: { // elevation
            let t = (in.world.z - u.z_min) / max(u.z_max - u.z_min, 1e-3);
            albedo = terrain(t);
        }
        case 2u: { // slope: 0 (flat) -> 90 deg (vertical)
            let slope = acos(clamp(nf.z, -1.0, 1.0)); // radians
            let t = clamp(slope / 1.5708, 0.0, 1.0);
            albedo = mix(vec3<f32>(0.2, 0.3, 0.7), vec3<f32>(0.9, 0.25, 0.2), t);
        }
        case 3u: { // aspect: compass direction of the slope
            let asp = degrees(atan2(nf.y, nf.x));
            let h = (asp + 360.0) % 360.0;
            albedo = hsv(h, 0.55, 0.95);
        }
        default: { // shaded grey
            albedo = vec3<f32>(0.72, 0.72, 0.72);
        }
    }

    let color = albedo * shade;
    return vec4<f32>(color, 1.0);
}
