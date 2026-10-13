// Passes rasterized in UV space: every texel of the texture set knows its
// world position and normal, so brush dabs can be tested in screen space.

struct DabParams {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x: viewport width (px), y: height (px), z: near, w: far
    viewport: vec4<f32>,
    // x: hardness, y: dab count
    brush: vec4<f32>,
};

@group(0) @binding(0) var<uniform> p: DabParams;
@group(0) @binding(1) var scene_depth: texture_depth_2d;
// xy: center (px), z: radius (px), w: strength
@group(0) @binding(2) var<storage, read> dabs: array<vec4<f32>>;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct UvOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_uv(v: VsIn) -> UvOut {
    var o: UvOut;
    o.pos = vec4<f32>(v.uv.x * 2.0 - 1.0, 1.0 - v.uv.y * 2.0, 0.0, 1.0);
    o.world = v.position;
    o.normal = v.normal;
    return o;
}

@fragment
fn fs_coverage(i: UvOut) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0);
}

@fragment
fn fs_dab(i: UvOut) -> @location(0) vec4<f32> {
    let clip = p.view_proj * vec4<f32>(i.world, 1.0);
    if (clip.w <= 0.0) { discard; }
    let ndc = clip.xyz / clip.w;
    let px = vec2<f32>((ndc.x * 0.5 + 0.5) * p.viewport.x, (0.5 - ndc.y * 0.5) * p.viewport.y);
    if (px.x < 0.0 || px.y < 0.0 || px.x >= p.viewport.x || px.y >= p.viewport.y) { discard; }

    // Occlusion: compare linear depths against the camera depth buffer.
    let near = p.viewport.z;
    let far = p.viewport.w;
    let d = textureLoad(scene_depth, vec2<i32>(px), 0);
    let scene_lin = near * far / (far - d * (far - near));
    if (clip.w > scene_lin * 1.005 + near * 2.0) { discard; }

    let facing = dot(normalize(i.normal), normalize(p.eye.xyz - i.world));
    if (facing <= 0.0) { discard; }

    var acc = 0.0;
    let count = u32(p.brush.y);
    for (var k = 0u; k < count; k = k + 1u) {
        let dab = dabs[k];
        let t = distance(px, dab.xy) / dab.z;
        let a = (1.0 - smoothstep(p.brush.x, 1.0, t)) * dab.w;
        acc = acc + (1.0 - acc) * a;
    }
    acc = acc * smoothstep(0.0, 0.15, facing);
    if (acc <= 0.0) { discard; }
    return vec4<f32>(acc, 0.0, 0.0, acc);
}

// Brush dabs in texture space (painting on the UV view): dab xy and radius in texels of the
// set; every covered texel of the triangles is tested against them.
@fragment
fn fs_dab_uv(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = frag.xy;
    var acc = 0.0;
    let count = u32(p.brush.y);
    for (var k = 0u; k < count; k = k + 1u) {
        let dab = dabs[k];
        let t = distance(px, dab.xy) / max(dab.z, 0.5);
        let a = (1.0 - smoothstep(p.brush.x, 1.0, t)) * dab.w;
        acc = acc + (1.0 - acc) * a;
    }
    if (acc <= 0.0) { discard; }
    return vec4<f32>(acc, 0.0, 0.0, acc);
}
