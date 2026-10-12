// Mask effect chain: one fullscreen pass per effect, in UV space.
// Noise is evaluated at the world position of each texel, so patterns are continuous
// across UV seams. Warps convert world-space offsets to UV offsets through the local
// derivative of the position map.

struct EffectParams {
    // x: effect (0 generator, 1 warp, 2 blur, 3 levels), y: mask blend, z: source / mode, w: noise kind
    kind: vec4<u32>,
    // low, high, gamma, steps
    levels: vec4<f32>,
    // x: opacity, y: invert, z: intensity (world), w: samples
    misc: vec4<f32>,
    // x: scale (world), y: octaves, z: stretch, w: seed
    noise: vec4<f32>,
    // xyz: direction, w: sharpness
    dir: vec4<f32>,
    bbox_min: vec4<f32>,
    bbox_size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> E: EffectParams;
@group(0) @binding(1) var prev: texture_2d<f32>;
@group(0) @binding(2) var lin: sampler;
@group(0) @binding(3) var geo_pos: texture_2d<f32>;   // world xyz, w coverage
@group(0) @binding(4) var geo_nrm: texture_2d<f32>;
@group(0) @binding(5) var map_ao: texture_2d<f32>;
@group(0) @binding(6) var map_curvature: texture_2d<f32>;
@group(0) @binding(7) var map_edges: texture_2d<f32>;
@group(0) @binding(8) var map_cavity: texture_2d<f32>;
@group(0) @binding(9) var map_thickness: texture_2d<f32>;
@group(0) @binding(10) var map_sky: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

// ---------------------------------------------------------------- noise

fn hash3(p: vec3<i32>, seed: u32) -> vec3<f32> {
    var h = vec3<u32>(bitcast<u32>(p.x), bitcast<u32>(p.y), bitcast<u32>(p.z));
    h = h * vec3<u32>(1597334673u, 3812015801u, 2798796415u) + vec3<u32>(seed * 747796405u);
    h = (h.x ^ h.y ^ h.z) * vec3<u32>(1597334673u, 3812015801u, 2798796415u);
    h = h ^ (h >> vec3<u32>(16u));
    h = h * vec3<u32>(2246822519u, 3266489917u, 668265263u);
    h = h ^ (h >> vec3<u32>(13u));
    return vec3<f32>(h & vec3<u32>(0xffffffu)) / 16777215.0;
}

fn fade(t: vec3<f32>) -> vec3<f32> {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

// Gradient noise in [-1, 1].
fn gnoise(p: vec3<f32>, seed: u32) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = p - floor(p);
    let u = fade(f);
    var acc = 0.0;
    var res: array<f32, 8>;
    for (var k = 0u; k < 8u; k = k + 1u) {
        let o = vec3<i32>(i32(k & 1u), i32((k >> 1u) & 1u), i32((k >> 2u) & 1u));
        let g = normalize(hash3(i + o, seed) * 2.0 - 1.0 + vec3<f32>(1e-4));
        res[k] = dot(g, f - vec3<f32>(o));
    }
    let x0 = mix(res[0], res[1], u.x);
    let x1 = mix(res[2], res[3], u.x);
    let x2 = mix(res[4], res[5], u.x);
    let x3 = mix(res[6], res[7], u.x);
    return mix(mix(x0, x1, u.y), mix(x2, x3, u.y), u.z) * 1.6;
}

fn fbm(p: vec3<f32>, octaves: u32, seed: u32) -> f32 {
    var sum = 0.0;
    var amp = 0.5;
    var norm = 0.0;
    var q = p;
    for (var o = 0u; o < max(octaves, 1u); o = o + 1u) {
        sum = sum + gnoise(q, seed + o * 7919u) * amp;
        norm = norm + amp;
        amp = amp * 0.5;
        q = q * 2.03 + vec3<f32>(17.1, 3.7, 9.2);
    }
    return sum / norm;
}

// Worley F2 - F1, roughly 0..1.
fn cellular(p: vec3<f32>, seed: u32) -> f32 {
    let i = vec3<i32>(floor(p));
    var f1 = 9.0;
    var f2 = 9.0;
    for (var z = -1; z <= 1; z = z + 1) {
        for (var y = -1; y <= 1; y = y + 1) {
            for (var x = -1; x <= 1; x = x + 1) {
                let c = i + vec3<i32>(x, y, z);
                let d = distance(p, vec3<f32>(c) + hash3(c, seed));
                if (d < f1) { f2 = f1; f1 = d; } else if (d < f2) { f2 = d; }
            }
        }
    }
    return clamp(f2 - f1, 0.0, 1.0);
}

// Local surface direction used to stretch stroke noise (wraps around the up axis).
fn stroke_axis(n: vec3<f32>) -> vec3<f32> {
    var t = cross(n, vec3<f32>(0.0, 1.0, 0.0));
    if (dot(t, t) < 1e-6) { t = cross(n, vec3<f32>(1.0, 0.0, 0.0)); }
    return normalize(t);
}

// Noise in [0, 1] at world position p with normal n.
fn noise01(p: vec3<f32>, n: vec3<f32>, kind: u32, offset: f32) -> f32 {
    let scale = max(E.noise.x, 1e-6);
    let seed = u32(E.noise.w);
    var q = p / scale + vec3<f32>(offset * 31.7);
    if (kind == 1u) {
        return cellular(q, seed);
    }
    if (kind == 2u) {
        let t = stroke_axis(n);
        q = q - t * dot(q, t) * (1.0 - 1.0 / max(E.noise.z, 1.0));
    }
    return clamp(fbm(q, u32(E.noise.y), seed) * 0.5 + 0.5, 0.0, 1.0);
}

// ---------------------------------------------------------------- helpers

fn levels(v: f32) -> f32 {
    var x = clamp((v - E.levels.x) / max(E.levels.y - E.levels.x, 1e-4), 0.0, 1.0);
    x = pow(x, 1.0 / max(E.levels.z, 1e-3));
    let steps = E.levels.w;
    if (steps >= 1.0) {
        x = min(floor(x * steps) / max(steps - 1.0, 1.0), 1.0);
    }
    if (E.misc.y > 0.5) { x = 1.0 - x; }
    return x;
}

fn combine(base: f32, v: f32) -> f32 {
    var r = v;
    switch E.kind.y {
        case 1u: { r = base * v; }
        case 2u: { r = base + v; }
        case 3u: { r = base - v; }
        case 4u: { r = max(base, v); }
        case 5u: { r = min(base, v); }
        default: { r = v; }
    }
    return clamp(mix(base, r, E.misc.x), 0.0, 1.0);
}

fn covered(xy: vec2<i32>) -> bool {
    let size = vec2<i32>(textureDimensions(geo_pos));
    if (any(xy < vec2<i32>(0)) || any(xy >= size)) { return false; }
    return textureLoad(geo_pos, xy, 0).w > 0.5;
}

fn world(xy: vec2<i32>) -> vec3<f32> {
    return textureLoad(geo_pos, xy, 0).xyz;
}

// Derivatives of world position per texel along u and v (central differences when possible).
fn jacobian(xy: vec2<i32>) -> mat2x3<f32> {
    let p = world(xy);
    var pu = vec3<f32>(0.0);
    var pv = vec3<f32>(0.0);
    let r = vec2<i32>(1, 0);
    let d = vec2<i32>(0, 1);
    if (covered(xy + r) && covered(xy - r)) { pu = (world(xy + r) - world(xy - r)) * 0.5; }
    else if (covered(xy + r)) { pu = world(xy + r) - p; }
    else if (covered(xy - r)) { pu = p - world(xy - r); }
    if (covered(xy + d) && covered(xy - d)) { pv = (world(xy + d) - world(xy - d)) * 0.5; }
    else if (covered(xy + d)) { pv = world(xy + d) - p; }
    else if (covered(xy - d)) { pv = p - world(xy - d); }
    return mat2x3<f32>(pu, pv);
}

// World-space offset -> texel offset (least squares through the jacobian).
fn to_texels(j: mat2x3<f32>, o: vec3<f32>) -> vec2<f32> {
    let a = dot(j[0], j[0]);
    let b = dot(j[0], j[1]);
    let c = dot(j[1], j[1]);
    let det = a * c - b * b;
    if (abs(det) < 1e-20) { return vec2<f32>(0.0); }
    let r = vec2<f32>(dot(j[0], o), dot(j[1], o));
    return vec2<f32>(c * r.x - b * r.y, a * r.y - b * r.x) / det;
}

fn sample_prev(px: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(prev));
    return textureSampleLevel(prev, lin, (px + 0.5) / size, 0.0).r;
}

// Noise flow direction in the tangent plane (gradient of the noise, rotated 90°
// for a swirling flow when `swirl` is true).
fn flow(p: vec3<f32>, n: vec3<f32>, kind: u32) -> vec3<f32> {
    let h = max(E.noise.x, 1e-6) * 0.15;
    let base = noise01(p, n, kind, 0.0);
    let g = vec3<f32>(
        noise01(p + vec3<f32>(h, 0.0, 0.0), n, kind, 0.0) - base,
        noise01(p + vec3<f32>(0.0, h, 0.0), n, kind, 0.0) - base,
        noise01(p + vec3<f32>(0.0, 0.0, h), n, kind, 0.0) - base,
    );
    let tangent = g - n * dot(g, n);
    let len = length(tangent);
    if (len < 1e-8) { return vec3<f32>(0.0); }
    return cross(n, tangent / len);
}

// ---------------------------------------------------------------- effects

fn generator(xy: vec2<i32>, p: vec3<f32>, n: vec3<f32>) -> f32 {
    let src = E.kind.z;
    switch src {
        case 0u: { return textureLoad(map_ao, xy, 0).r; }
        case 1u: { return textureLoad(map_curvature, xy, 0).r; }
        case 2u: { return textureLoad(map_edges, xy, 0).r; }
        case 3u: { return textureLoad(map_cavity, xy, 0).r; }
        case 4u: { return textureLoad(map_thickness, xy, 0).r; }
        case 5u: { return textureLoad(map_sky, xy, 0).r; }
        case 10u, 11u, 12u: {
            let axis = src - 10u;
            return clamp((p[axis] - E.bbox_min[axis]) / max(E.bbox_size[axis], 1e-6), 0.0, 1.0);
        }
        default: { return noise01(p, n, E.kind.w, 0.0); }
    }
}

fn warp(xy: vec2<i32>, p: vec3<f32>, n: vec3<f32>) -> f32 {
    let mode = E.kind.z;
    let kind = E.kind.w;
    let dist = E.misc.z;
    let here = vec2<f32>(xy);
    if (mode == 0u) {
        // Displace by a vector noise field projected on the surface.
        var o = vec3<f32>(
            noise01(p, n, kind, 0.0),
            noise01(p, n, kind, 1.0),
            noise01(p, n, kind, 2.0),
        ) * 2.0 - 1.0;
        o = (o - n * dot(o, n)) * dist;
        let dest = here + to_texels(jacobian(xy), o);
        if (!covered(vec2<i32>(round(dest)))) { return sample_prev(here); }
        return sample_prev(dest);
    }
    // Slope blur / directional: walk across the surface, accumulating the mask.
    let steps = max(u32(E.misc.w), 1u);
    let step_len = dist / f32(steps);
    var pos = here;
    var wp = p;
    var wn = n;
    var sum = sample_prev(here);
    var mx = sum;
    var count = 1.0;
    for (var i = 0u; i < steps; i = i + 1u) {
        var d: vec3<f32>;
        if (mode == 1u) {
            d = flow(wp, wn, kind);
        } else {
            let along = normalize(E.dir.xyz + vec3<f32>(1e-6));
            d = along - wn * dot(along, wn);
            // Noise-modulated stroke length.
            d = d * (0.25 + 1.5 * noise01(p, n, kind, 3.0));
        }
        let next = pos + to_texels(jacobian(vec2<i32>(round(pos))), d * step_len);
        let ixy = vec2<i32>(round(next));
        if (!covered(ixy)) { break; }
        pos = next;
        wp = world(ixy);
        wn = normalize(textureLoad(geo_nrm, ixy, 0).xyz);
        let v = sample_prev(pos);
        sum = sum + v;
        mx = max(mx, v);
        count = count + 1.0;
    }
    return mix(sum / count, mx, E.dir.w);
}

fn blur(xy: vec2<i32>) -> f32 {
    let r = max(E.misc.z, 0.0);
    if (r < 0.5) { return textureLoad(prev, xy, 0).r; }
    let stride = max(r / 6.0, 1.0);
    let sigma = r * 0.5;
    var sum = 0.0;
    var wsum = 0.0;
    for (var y = -6; y <= 6; y = y + 1) {
        for (var x = -6; x <= 6; x = x + 1) {
            let o = vec2<f32>(f32(x), f32(y)) * stride;
            let q = vec2<i32>(round(vec2<f32>(xy) + o));
            if (!covered(q)) { continue; }
            let w = exp(-dot(o, o) / (2.0 * sigma * sigma));
            sum = sum + textureLoad(prev, q, 0).r * w;
            wsum = wsum + w;
        }
    }
    return sum / max(wsum, 1e-6);
}

@fragment
fn fs_effect(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(frag.xy);
    let base = textureLoad(prev, xy, 0).r;
    let gp = textureLoad(geo_pos, xy, 0);
    if (gp.w < 0.5) { return vec4<f32>(base); }
    let p = gp.xyz;
    let n = normalize(textureLoad(geo_nrm, xy, 0).xyz);
    var out = base;
    switch E.kind.x {
        case 0u: { out = combine(base, levels(generator(xy, p, n))); }
        case 1u: { out = warp(xy, p, n); }
        case 2u: { out = blur(xy); }
        default: { out = levels(base); }
    }
    return vec4<f32>(out);
}
