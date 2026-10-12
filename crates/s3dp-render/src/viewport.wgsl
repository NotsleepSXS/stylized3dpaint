struct Globals {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x: shading mode, y: checker scale
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var paint_tex: texture_2d<f32>;
@group(1) @binding(1) var paint_sampler: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = g.view_proj * vec4<f32>(v.position, 1.0);
    o.world = v.position;
    o.normal = v.normal;
    o.uv = v.uv;
    return o;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn clay(n: vec3<f32>, v: vec3<f32>, base: vec3<f32>) -> vec3<f32> {
    let key = normalize(vec3<f32>(0.5, 0.8, 0.4));
    let fill = normalize(vec3<f32>(-0.6, 0.2, -0.5));
    let wrap = max(dot(n, key) * 0.5 + 0.5, 0.0);
    let sky = mix(vec3<f32>(0.25, 0.22, 0.2), vec3<f32>(0.55, 0.6, 0.7), n.y * 0.5 + 0.5);
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0) * 0.25;
    let h = normalize(key + v);
    let spec = pow(max(dot(n, h), 0.0), 32.0) * 0.15;
    return base * (wrap * wrap * vec3<f32>(1.0, 0.96, 0.9) + sky * 0.35 + max(dot(n, fill), 0.0) * 0.15) + rim + spec;
}

@fragment
fn fs_main(i: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(i.normal);
    if (!front) { n = -n; }
    let v = normalize(g.eye.xyz - i.world);
    let mode = u32(g.params.x);

    // Paint is stored premultiplied, sRGB-encoded.
    let p = textureSample(paint_tex, paint_sampler, i.uv);
    let painted = srgb_to_linear(p.rgb / max(p.a, 1e-4));
    let albedo = mix(vec3<f32>(0.5, 0.49, 0.47), painted, p.a);

    var col: vec3<f32>;
    if (mode == 1u) {
        col = albedo;
    } else if (mode == 2u) {
        let c = floor(i.uv * g.params.y);
        let check = (i32(c.x) + i32(c.y)) & 1;
        col = clay(n, v, select(vec3<f32>(0.25, 0.45, 0.85), vec3<f32>(0.85), check == 0));
    } else if (mode == 3u) {
        col = n * 0.5 + 0.5;
    } else {
        col = clay(n, v, albedo);
    }
    if (!front) { col *= vec3<f32>(0.6, 0.35, 0.35); }
    return vec4<f32>(col, 1.0);
}
