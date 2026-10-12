// Stylize filter layer: processes the composited channel below it.
// Color: painterly (Kuwahara) simplification, value bands, saturation, warm lights /
// cool shadows, edge highlights, cavity darkening. Roughness: simplification, bands and
// polished edges. Metallic: crisp threshold. Height: smoothing and rounded edge bevel.

struct StylizeUniform {
    // x: painterly radius, y: color bands, z: band strength, w: saturation
    a: vec4<f32>,
    // x: temperature, y: edge highlight, z: cavity darken, w: roughness bands
    b: vec4<f32>,
    // x: edge polish, y: metal threshold, z: height bevel, w: height smooth
    c: vec4<f32>,
    // x: channel (0 base color, 1 roughness, 2 metallic, 3 height), y: opacity, z: use mask
    d: vec4<f32>,
};

@group(0) @binding(0) var<uniform> S: StylizeUniform;
@group(0) @binding(1) var acc: texture_2d<f32>;
@group(0) @binding(2) var mask: texture_2d<f32>;
@group(0) @binding(3) var map_edges: texture_2d<f32>;
@group(0) @binding(4) var map_cavity: texture_2d<f32>;
@group(0) @binding(5) var map_ao: texture_2d<f32>;
@group(0) @binding(6) var map_sky: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

fn load(xy: vec2<i32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(acc));
    return textureLoad(acc, clamp(xy, vec2<i32>(0), size - 1), 0).rgb;
}

// Kuwahara filter: mean of the least varying quadrant — flat, painterly patches
// that keep edges sharp.
fn kuwahara(xy: vec2<i32>, radius: f32) -> vec3<f32> {
    let r = i32(clamp(radius, 0.0, 8.0));
    if (r < 1) { return load(xy); }
    var best = load(xy);
    var best_var = 1e9;
    for (var q = 0; q < 4; q = q + 1) {
        let sx = select(-1, 1, (q & 1) == 1);
        let sy = select(-1, 1, (q & 2) == 2);
        var sum = vec3<f32>(0.0);
        var sum2 = vec3<f32>(0.0);
        var n = 0.0;
        for (var y = 0; y <= r; y = y + 1) {
            for (var x = 0; x <= r; x = x + 1) {
                let c = load(xy + vec2<i32>(x * sx, y * sy));
                sum = sum + c;
                sum2 = sum2 + c * c;
                n = n + 1.0;
            }
        }
        let mean = sum / n;
        let v = sum2 / n - mean * mean;
        let total = v.x + v.y + v.z;
        if (total < best_var) {
            best_var = total;
            best = mean;
        }
    }
    return best;
}

fn blur(xy: vec2<i32>, radius: f32) -> f32 {
    let r = i32(clamp(radius, 0.0, 8.0));
    if (r < 1) { return load(xy).r; }
    var sum = 0.0;
    var n = 0.0;
    for (var y = -r; y <= r; y = y + 1) {
        for (var x = -r; x <= r; x = x + 1) {
            sum = sum + load(xy + vec2<i32>(x, y)).r;
            n = n + 1.0;
        }
    }
    return sum / n;
}

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let d = mx - mn;
    var h = 0.0;
    if (d > 1e-6) {
        if (mx == c.r) { h = (c.g - c.b) / d; }
        else if (mx == c.g) { h = (c.b - c.r) / d + 2.0; }
        else { h = (c.r - c.g) / d + 4.0; }
        h = fract(h / 6.0);
    }
    return vec3<f32>(h, select(0.0, d / mx, mx > 1e-6), mx);
}

fn hsv_to_rgb(c: vec3<f32>) -> vec3<f32> {
    let k = vec3<f32>(5.0, 3.0, 1.0);
    let t = (k + c.x * 6.0) % vec3<f32>(6.0);
    return c.z - c.z * c.y * clamp(min(t, 4.0 - t), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn bands(v: f32, count: f32, strength: f32) -> f32 {
    if (count < 1.0) { return v; }
    let q = (floor(v * count) + 0.5) / count;
    return mix(v, q, strength);
}

@fragment
fn fs_stylize(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(frag.xy);
    let src = load(xy);
    let edges = textureLoad(map_edges, xy, 0).r;
    let cavity = textureLoad(map_cavity, xy, 0).r;
    var k = S.d.y;
    if (S.d.z > 0.5) { k = k * textureLoad(mask, xy, 0).r; }
    var out = src;
    switch u32(S.d.x) {
        case 0u: {
            var c = kuwahara(xy, S.a.x);
            var hsv = rgb_to_hsv(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)));
            hsv.z = bands(hsv.z, S.a.y, S.a.z);
            hsv.y = clamp(hsv.y * S.a.w, 0.0, 1.0);
            c = hsv_to_rgb(hsv);
            // Warm lights, cool shadows from the baked lighting.
            let light = textureLoad(map_sky, xy, 0).r * textureLoad(map_ao, xy, 0).r;
            let tint = mix(vec3<f32>(0.82, 0.88, 1.12), vec3<f32>(1.1, 1.0, 0.86), light);
            c = c * mix(vec3<f32>(1.0), tint, S.b.x);
            c = mix(c, c * 1.35 + 0.08, edges * S.b.y);
            c = c * (1.0 - cavity * S.b.z);
            out = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
        }
        case 1u: {
            var r = kuwahara(xy, S.a.x).r;
            r = bands(r, S.b.w, 1.0);
            r = r * (1.0 - edges * S.c.x);
            out = vec3<f32>(r);
        }
        case 2u: {
            let t = S.c.y;
            out = vec3<f32>(smoothstep(t - 0.01, t + 0.01, src.r));
        }
        default: {
            var h = blur(xy, S.c.w);
            h = h - edges * S.c.z * 0.25 - cavity * S.c.z * 0.1;
            out = vec3<f32>(clamp(h, 0.0, 1.0));
        }
    }
    return vec4<f32>(mix(src, out, clamp(k, 0.0, 1.0)), 1.0);
}
