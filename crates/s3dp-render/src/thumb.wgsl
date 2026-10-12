// Small previews of layer textures for the layers panel (box-filtered).

struct ThumbParams {
    // x: mode (0 color premult, 1 scalar (value, a), 2 mask), y: taps per axis
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> t: ThumbParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> Out {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: Out;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

@fragment
fn fs_thumb(i: Out) -> @location(0) vec4<f32> {
    let n = i32(t.params.y);
    let texel = 1.0 / vec2<f32>(textureDimensions(src));
    let px = fwidth(i.uv);
    var acc = vec4<f32>(0.0);
    for (var y = 0; y < n; y = y + 1) {
        for (var x = 0; x < n; x = x + 1) {
            let o = (vec2<f32>(f32(x), f32(y)) + 0.5) / f32(n) - 0.5;
            acc = acc + textureSampleLevel(src, samp, i.uv + o * px, 0.0);
        }
    }
    let v = acc / f32(n * n);
    let mode = u32(t.params.x);
    // Output is linear premultiplied for an Rgba8UnormSrgb target (what egui expects).
    var straight: vec4<f32>;
    if (mode == 1u) {
        straight = vec4<f32>(vec3<f32>(v.r / max(v.g, 1e-4)), v.g);
    } else if (mode == 2u) {
        straight = vec4<f32>(v.rrr, 1.0);
    } else {
        straight = vec4<f32>(v.rgb / max(v.a, 1e-4), v.a);
    }
    return vec4<f32>(srgb_to_linear(clamp(straight.rgb, vec3<f32>(0.0), vec3<f32>(1.0))) * straight.a, straight.a);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}
