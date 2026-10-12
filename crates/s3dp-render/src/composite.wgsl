// Applies the accumulated stroke mask on top of the pre-stroke paint texture.
// Content channels: Rgba16Float premultiplied linear (rgb * a, a); scalar channels replicate
// the value in rgb. Mask paint items: Rg16Float premultiplied (value * a, a).

struct CompositeParams {
    // rgb: brush value (linear)
    value: vec4<f32>,
    // x: opacity, y: mode (0 rgba over, 1 rg over (mask paint), 2 erase)
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> c: CompositeParams;
@group(0) @binding(1) var before: texture_2d<f32>;
@group(0) @binding(2) var mask: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pos.xy);
    let dst = textureLoad(before, xy, 0);
    let m = clamp(textureLoad(mask, xy, 0).r * c.params.x, 0.0, 1.0);
    let mode = u32(c.params.y);
    if (mode == 2u) {
        return dst * (1.0 - m);
    }
    if (mode == 1u) {
        return vec4<f32>(c.value.r * m + dst.r * (1.0 - m), m + dst.g * (1.0 - m), 0.0, 0.0);
    }
    return vec4<f32>(c.value.rgb, 1.0) * m + dst * (1.0 - m);
}
