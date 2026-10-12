// UV-space G-buffer of the low-poly mesh for the baker (two passes because of the
// per-sample color attachment byte limit).

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tangent: vec4<f32>,
};

@vertex
fn vs_uv(v: VsIn) -> VsOut {
    var o: VsOut;
    o.pos = vec4<f32>(v.uv.x * 2.0 - 1.0, 1.0 - v.uv.y * 2.0, 0.0, 1.0);
    o.world = v.position;
    o.normal = v.normal;
    o.tangent = v.tangent;
    return o;
}

struct PassA {
    @location(0) pos: vec4<f32>,
    @location(1) normal: vec4<f32>,
};

@fragment
fn fs_a(i: VsOut) -> PassA {
    var o: PassA;
    o.pos = vec4<f32>(i.world, 1.0);
    o.normal = vec4<f32>(normalize(i.normal), 1.0);
    return o;
}

struct PassB {
    @location(0) tangent: vec4<f32>,
    @location(1) face: vec4<f32>,
};

@fragment
fn fs_b(i: VsOut) -> PassB {
    var o: PassB;
    o.tangent = i.tangent;
    // Geometric normal from screen-space (= UV-space) derivatives of the position.
    var f = cross(dpdx(i.world), dpdy(i.world));
    let len = length(f);
    f = select(normalize(i.normal), f / max(len, 1e-20), len > 1e-20);
    if (dot(f, i.normal) < 0.0) { f = -f; }
    o.face = vec4<f32>(f, 1.0);
    return o;
}
