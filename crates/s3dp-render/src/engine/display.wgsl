// Final channel -> display texture (formats per channel), dilated past UV island borders so
// bilinear filtering and mipmaps never pick up the background.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var geo_pos: texture_2d<f32>;

const RADIUS: i32 = 8;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_display(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pos.xy);
    if (textureLoad(geo_pos, xy, 0).w > 0.5) {
        return vec4<f32>(textureLoad(src, xy, 0).rgb, 1.0);
    }
    let last = vec2<i32>(textureDimensions(src)) - 1;
    var best = textureLoad(src, xy, 0);
    var best_d = 1e9;
    for (var dy = -RADIUS; dy <= RADIUS; dy = dy + 1) {
        for (var dx = -RADIUS; dx <= RADIUS; dx = dx + 1) {
            let q = clamp(xy + vec2<i32>(dx, dy), vec2<i32>(0), last);
            let d = f32(dx * dx + dy * dy);
            if (d < best_d && textureLoad(geo_pos, q, 0).w > 0.5) {
                best_d = d;
                best = textureLoad(src, q, 0);
            }
        }
    }
    return vec4<f32>(best.rgb, 1.0);
}
