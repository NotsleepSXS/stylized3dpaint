// Copies a composited channel to its display texture, pushing island borders
// outwards into empty UV space so bilinear/mip sampling has no seams.

const RADIUS: i32 = 6;

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var coverage: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_dilate(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pos.xy);
    if (textureLoad(coverage, xy, 0).r > 0.5) {
        return textureLoad(src, xy, 0);
    }
    let last = vec2<i32>(textureDimensions(src)) - 1;
    var best = vec4<f32>(0.0);
    var best_d = 1e9;
    for (var dy = -RADIUS; dy <= RADIUS; dy = dy + 1) {
        for (var dx = -RADIUS; dx <= RADIUS; dx = dx + 1) {
            let q = clamp(xy + vec2<i32>(dx, dy), vec2<i32>(0), last);
            let d = f32(dx * dx + dy * dy);
            if (d < best_d && textureLoad(coverage, q, 0).r > 0.5) {
                best_d = d;
                best = textureLoad(src, q, 0);
            }
        }
    }
    return best;
}
