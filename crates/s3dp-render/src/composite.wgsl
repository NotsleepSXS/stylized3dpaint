// Applies the accumulated stroke mask on top of the pre-stroke texture.
// Paint textures store premultiplied sRGB-encoded color.

struct CompositeParams {
    color: vec4<f32>,
    // x: opacity, y: erase (0/1)
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
    if (c.params.y > 0.5) {
        return dst * (1.0 - m);
    }
    return vec4<f32>(c.color.rgb, 1.0) * m + dst * (1.0 - m);
}
