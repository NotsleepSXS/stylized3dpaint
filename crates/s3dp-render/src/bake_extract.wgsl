// Turns baker outputs into final display maps, dilating past UV island borders.

struct ExtractParams {
    // x: mode (0 scalar channel, 1 rgb copy, 2 normal encode, 3 position normalize), y: channel
    mode: vec4<f32>,
    bbox_min: vec4<f32>,
    bbox_size: vec4<f32>,
};

const RADIUS: i32 = 6;

@group(0) @binding(0) var<uniform> e: ExtractParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var coverage: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

fn value(xy: vec2<i32>) -> vec4<f32> {
    let s = textureLoad(src, xy, 0);
    let mode = u32(e.mode.x);
    if (mode == 0u) {
        let c = s[u32(e.mode.y)];
        return vec4<f32>(c, c, c, 1.0);
    }
    if (mode == 2u) {
        return vec4<f32>(normalize(s.xyz) * 0.5 + 0.5, 1.0);
    }
    if (mode == 3u) {
        return vec4<f32>((s.xyz - e.bbox_min.xyz) / max(e.bbox_size.xyz, vec3<f32>(1e-9)), 1.0);
    }
    return vec4<f32>(s.xyz, 1.0);
}

@fragment
fn fs_extract(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pos.xy);
    if (textureLoad(coverage, xy, 0).w > 0.5) {
        return value(xy);
    }
    let last = vec2<i32>(textureDimensions(src)) - 1;
    var best = vec4<f32>(0.0);
    var best_d = 1e9;
    for (var dy = -RADIUS; dy <= RADIUS; dy = dy + 1) {
        for (var dx = -RADIUS; dx <= RADIUS; dx = dx + 1) {
            let q = clamp(xy + vec2<i32>(dx, dy), vec2<i32>(0), last);
            let d = f32(dx * dx + dy * dy);
            if (d < best_d && textureLoad(coverage, q, 0).w > 0.5) {
                best_d = d;
                best = value(q);
            }
        }
    }
    return best;
}
