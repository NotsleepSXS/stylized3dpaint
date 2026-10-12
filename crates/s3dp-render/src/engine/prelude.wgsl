// ============================================================================================
// Effect prelude: shared by every effect shader (spec §2.5).
//
// An effect file defines
//     fn effect(c: Ctx) -> vec4<f32>
// and reads its parameters through generated accessors `p_<id>()`. The engine appends a
// `fs_main` that builds the Ctx, calls `effect`, applies the generator post-processing
// (generators only) and blends the result over the input with the instance blend mode and
// opacity.
//
// Values are LINEAR. Content passes see one channel at a time: color channels in rgb,
// scalar channels in r (rgb replicated), alpha = coverage. Mask passes see the mask in r.
// All distances given to effects are in object units: multiply relative parameters by
// `radius()`; noise is evaluated at world positions, so results have no UV seams and do
// not depend on the texture resolution.
// ============================================================================================

struct Uniforms {
    params: array<vec4<f32>, 48>,
    // x: channel id, y: channel is color (0/1), z: target (0 content, 1 mask), w: generator (0/1)
    info: vec4<u32>,
    // x: blend mode, y: opacity, z: anchor0 bound, w: anchor1 bound
    blend: vec4<f32>,
    // x: object radius, y: seed, z: time-independent salt, w: seamless (follow UV seam links)
    scale: vec4<f32>,
    bbox_min: vec4<f32>,
    bbox_size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> U: Uniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var lin: sampler;
@group(0) @binding(3) var geo_pos: texture_2d<f32>;      // world xyz, w = coverage (not filterable)
@group(0) @binding(4) var geo_nrm: texture_2d<f32>;
@group(0) @binding(5) var map_ao: texture_2d<f32>;
@group(0) @binding(6) var map_curvature: texture_2d<f32>;
@group(0) @binding(7) var map_edges: texture_2d<f32>;
@group(0) @binding(8) var map_cavity: texture_2d<f32>;
@group(0) @binding(9) var map_thickness: texture_2d<f32>;
@group(0) @binding(10) var map_sky: texture_2d<f32>;
@group(0) @binding(11) var map_normal: texture_2d<f32>;  // baked tangent-space normal, encoded 0..1
@group(0) @binding(12) var anchor_tex0: texture_2d<f32>;
@group(0) @binding(13) var anchor_tex1: texture_2d<f32>;
@group(0) @binding(14) var seam_link_tex: texture_2d<f32>; // texel across the UV seam, -1 = none (not filterable)

const PI: f32 = 3.14159265;

struct Ctx {
    xy: vec2<i32>,
    // texel center in 0..1
    uv: vec2<f32>,
    size: vec2<i32>,
    // world position / normal
    p: vec3<f32>,
    n: vec3<f32>,
    // input value at this texel
    src: vec4<f32>,
    covered: bool,
};

// ---------------------------------------------------------------- context helpers

fn radius() -> f32 { return U.scale.x; }
fn seed() -> u32 { return u32(max(U.scale.y, 0.0)); }
fn channel() -> u32 { return U.info.x; }
fn is_color_channel() -> bool { return U.info.y == 1u; }
fn is_mask() -> bool { return U.info.z == 1u; }

fn tex_size() -> vec2<i32> { return vec2<i32>(textureDimensions(geo_pos)); }

fn covered(xy: vec2<i32>) -> bool {
    let s = tex_size();
    if (any(xy < vec2<i32>(0)) || any(xy >= s)) { return false; }
    return textureLoad(geo_pos, xy, 0).w > 0.5;
}

fn world(xy: vec2<i32>) -> vec3<f32> { return textureLoad(geo_pos, clamp(xy, vec2<i32>(0), tex_size() - 1), 0).xyz; }
fn world_normal(xy: vec2<i32>) -> vec3<f32> {
    return normalize(textureLoad(geo_nrm, clamp(xy, vec2<i32>(0), tex_size() - 1), 0).xyz + vec3<f32>(0.0, 1e-6, 0.0));
}

/// Position normalized to the object bounding box (0..1).
fn position01(p: vec3<f32>) -> vec3<f32> { return clamp((p - U.bbox_min.xyz) / max(U.bbox_size.xyz, vec3<f32>(1e-6)), vec3<f32>(0.0), vec3<f32>(1.0)); }

fn load_src(xy: vec2<i32>) -> vec4<f32> { return textureLoad(src_tex, clamp(xy, vec2<i32>(0), tex_size() - 1), 0); }
/// Bilinear sample of the input at a (fractional) texel position.
fn sample_src_px(px: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(src_tex, lin, (px + 0.5) / vec2<f32>(tex_size()), 0.0);
}

fn map_load(t: texture_2d<f32>, xy: vec2<i32>) -> f32 { return textureLoad(t, clamp(xy, vec2<i32>(0), tex_size() - 1), 0).r; }
fn ao(xy: vec2<i32>) -> f32 { return map_load(map_ao, xy); }
fn curvature(xy: vec2<i32>) -> f32 { return map_load(map_curvature, xy); }
fn edges(xy: vec2<i32>) -> f32 { return map_load(map_edges, xy); }
fn cavity(xy: vec2<i32>) -> f32 { return map_load(map_cavity, xy); }
fn thickness(xy: vec2<i32>) -> f32 { return map_load(map_thickness, xy); }
fn sky(xy: vec2<i32>) -> f32 { return map_load(map_sky, xy); }
fn baked_normal(xy: vec2<i32>) -> vec3<f32> {
    return textureLoad(map_normal, clamp(xy, vec2<i32>(0), tex_size() - 1), 0).xyz * 2.0 - 1.0;
}

fn anchor0(xy: vec2<i32>) -> vec4<f32> { return textureLoad(anchor_tex0, clamp(xy, vec2<i32>(0), vec2<i32>(textureDimensions(anchor_tex0)) - 1), 0); }
fn anchor1(xy: vec2<i32>) -> vec4<f32> { return textureLoad(anchor_tex1, clamp(xy, vec2<i32>(0), vec2<i32>(textureDimensions(anchor_tex1)) - 1), 0); }
fn has_anchor0() -> bool { return U.blend.z > 0.5; }
fn has_anchor1() -> bool { return U.blend.w > 0.5; }

// Derivatives of the world position per texel along u and v.
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

/// World-space offset -> texel offset (least squares through the jacobian).
fn to_texels(j: mat2x3<f32>, o: vec3<f32>) -> vec2<f32> {
    let a = dot(j[0], j[0]);
    let b = dot(j[0], j[1]);
    let c = dot(j[1], j[1]);
    let det = a * c - b * b;
    if (abs(det) < 1e-24) { return vec2<f32>(0.0); }
    let r = vec2<f32>(dot(j[0], o), dot(j[1], o));
    return vec2<f32>(c * r.x - b * r.y, a * r.y - b * r.x) / det;
}

// ---------------------------------------------------------------- seam-aware neighbours

/// Seamless mode: neighbour lookups continue across UV seams via the seam links.
fn seamless() -> bool { return U.scale.w > 0.5; }

/// Texel on the other side of the seam for a border texel (x < 0: none).
fn seam_link(xy: vec2<i32>) -> vec2<i32> {
    let l = textureLoad(seam_link_tex, clamp(xy, vec2<i32>(0), tex_size() - 1), 0).xy;
    return vec2<i32>(round(l));
}

/// A linked border texel at or around `b` (within 2 texels), or x < 0.
fn find_link(b: vec2<i32>) -> vec4<i32> {
    var best = vec4<i32>(-1);
    var best_d = 99;
    for (var y = -2; y <= 2; y = y + 1) {
        for (var x = -2; x <= 2; x = x + 1) {
            let q = b + vec2<i32>(x, y);
            if (!covered(q)) { continue; }
            let l = seam_link(q);
            let d = abs(x) + abs(y);
            if (l.x >= 0 && d < best_d) {
                best_d = d;
                best = vec4<i32>(q, l);
            }
        }
    }
    return best;
}

/// Texel position of the surface point `p + o` seen from the (fractional) texel `start` whose
/// world position is `p`; `o` is a world offset (tangent to the surface). Returns
/// (px.x, px.y, ok). In UV mode only the island of `start` is used (ok = 0 when the offset
/// leaves it); in seamless mode the lookup jumps across the UV seam through the seam links.
fn surface_px_f(start: vec2<f32>, p: vec3<f32>, o: vec3<f32>) -> vec3<f32> {
    let sxy = vec2<i32>(round(start));
    let j = jacobian(sxy);
    let tw = texel_world(sxy);
    let len = length(o);
    let px = start + to_texels(j, o);
    let ixy = vec2<i32>(round(px));
    if (covered(ixy) && distance(world(ixy), p) <= len * 2.0 + tw * 2.0) { return vec3<f32>(px, 1.0); }
    if (!seamless()) { return vec3<f32>(px, 0.0); }
    // Last texel of our island along the way (binary search).
    var lo = 0.0;
    var hi = 1.0;
    for (var i = 0; i < 7; i = i + 1) {
        let mid = 0.5 * (lo + hi);
        let m = vec2<i32>(round(mix(start, px, mid)));
        if (covered(m) && distance(world(m), p) <= len * mid * 2.0 + tw * 2.0) { lo = mid; } else { hi = mid; }
    }
    let b = vec2<i32>(round(mix(start, px, lo)));
    let link = find_link(b);
    if (link.x < 0) { return vec3<f32>(px, 0.0); }
    // Continue the rest of the offset on the other side, in its own UV frame.
    let target_p = p + o;
    let rest = target_p - world(link.xy);
    let px2 = vec2<f32>(link.zw) + to_texels(jacobian(link.zw), rest);
    let i2 = vec2<i32>(round(px2));
    if (covered(i2) && distance(world(i2), target_p) <= length(rest) * 0.75 + tw * 3.0) { return vec3<f32>(px2, 1.0); }
    // Overshot the other island too (corner, tiny island): stop right across the seam.
    if (length(rest) <= tw * 3.0) { return vec3<f32>(vec2<f32>(link.zw), 1.0); }
    return vec3<f32>(px, 0.0);
}

fn surface_px(xy: vec2<i32>, p: vec3<f32>, o: vec3<f32>) -> vec3<f32> {
    return surface_px_f(vec2<f32>(xy), p, o);
}

/// Size of one texel in world units (average of both directions).
fn texel_world(xy: vec2<i32>) -> f32 {
    let j = jacobian(xy);
    return max(0.5 * (length(j[0]) + length(j[1])), 1e-9);
}

/// Orthonormal tangent basis around n (columns: t, b, n).
fn basis(n: vec3<f32>) -> mat3x3<f32> {
    let s = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    return mat3x3<f32>(vec3<f32>(1.0 + s * n.x * n.x * a, s * b, -s * n.x), vec3<f32>(b, s + n.y * n.y * a, -n.y), n);
}

// ---------------------------------------------------------------- color

fn luminance(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }

fn srgb_to_linear3(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}
fn linear_to_srgb3(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x * 12.92, x <= vec3<f32>(0.0031308));
}

fn rgb_to_hsv(c: vec3<f32>) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let d = mx - mn;
    var h = 0.0;
    if (d > 1e-6) {
        if (mx == c.r) { h = (c.g - c.b) / d; } else if (mx == c.g) { h = (c.b - c.r) / d + 2.0; } else { h = (c.r - c.g) / d + 4.0; }
        h = fract(h / 6.0);
    }
    return vec3<f32>(h, select(0.0, d / mx, mx > 1e-6), mx);
}

fn hsv_to_rgb(c: vec3<f32>) -> vec3<f32> {
    let t = (vec3<f32>(5.0, 3.0, 1.0) + c.x * 6.0) % vec3<f32>(6.0);
    return c.z - c.z * c.y * clamp(min(t, 4.0 - t), vec3<f32>(0.0), vec3<f32>(1.0));
}

// ---------------------------------------------------------------- noise (3D, world space)

fn hash3(p: vec3<i32>, s: u32) -> vec3<f32> {
    var h = vec3<u32>(bitcast<u32>(p.x), bitcast<u32>(p.y), bitcast<u32>(p.z));
    h = h * vec3<u32>(1597334673u, 3812015801u, 2798796415u) + vec3<u32>(s * 747796405u + 1u);
    h = (h.x ^ h.y ^ h.z) * vec3<u32>(1597334673u, 3812015801u, 2798796415u);
    h = h ^ (h >> vec3<u32>(16u));
    h = h * vec3<u32>(2246822519u, 3266489917u, 668265263u);
    h = h ^ (h >> vec3<u32>(13u));
    return vec3<f32>(h & vec3<u32>(0xffffffu)) / 16777215.0;
}

fn hash1(p: vec3<i32>, s: u32) -> f32 { return hash3(p, s).x; }

fn fade(t: vec3<f32>) -> vec3<f32> { return t * t * t * (t * (t * 6.0 - 15.0) + 10.0); }

/// Gradient noise, roughly -1..1.
fn gnoise(p: vec3<f32>, s: u32) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = p - floor(p);
    let u = fade(f);
    var r: array<f32, 8>;
    for (var k = 0u; k < 8u; k = k + 1u) {
        let o = vec3<i32>(i32(k & 1u), i32((k >> 1u) & 1u), i32((k >> 2u) & 1u));
        let g = normalize(hash3(i + o, s) * 2.0 - 1.0 + vec3<f32>(1e-4));
        r[k] = dot(g, f - vec3<f32>(o));
    }
    let x0 = mix(r[0], r[1], u.x);
    let x1 = mix(r[2], r[3], u.x);
    let x2 = mix(r[4], r[5], u.x);
    let x3 = mix(r[6], r[7], u.x);
    return mix(mix(x0, x1, u.y), mix(x2, x3, u.y), u.z) * 1.6;
}

/// Fractal noise in 0..1. `scale` is the feature size in world units.
fn fbm01(p: vec3<f32>, scale: f32, octaves: i32, s: u32) -> f32 {
    var q = p / max(scale, 1e-6);
    var sum = 0.0;
    var amp = 0.5;
    var norm = 0.0;
    for (var o = 0; o < max(octaves, 1); o = o + 1) {
        sum = sum + gnoise(q, s + u32(o) * 7919u) * amp;
        norm = norm + amp;
        amp = amp * 0.5;
        q = q * 2.03 + vec3<f32>(17.1, 3.7, 9.2);
    }
    return clamp(sum / norm * 0.5 + 0.5, 0.0, 1.0);
}

/// Worley distances (F1, F2) in cell units.
fn worley(q: vec3<f32>, s: u32) -> vec2<f32> {
    let i = vec3<i32>(floor(q));
    var f1 = 9.0;
    var f2 = 9.0;
    for (var z = -1; z <= 1; z = z + 1) {
        for (var y = -1; y <= 1; y = y + 1) {
            for (var x = -1; x <= 1; x = x + 1) {
                let c = i + vec3<i32>(x, y, z);
                let d = distance(q, vec3<f32>(c) + hash3(c, s));
                if (d < f1) { f2 = f1; f1 = d; } else if (d < f2) { f2 = d; }
            }
        }
    }
    return vec2<f32>(f1, f2);
}

/// Random value per Worley cell (stable id for chips / spots).
fn cell_id(q: vec3<f32>, s: u32) -> f32 {
    let i = vec3<i32>(floor(q));
    var best = 9.0;
    var id = 0.0;
    for (var z = -1; z <= 1; z = z + 1) {
        for (var y = -1; y <= 1; y = y + 1) {
            for (var x = -1; x <= 1; x = x + 1) {
                let c = i + vec3<i32>(x, y, z);
                let d = distance(q, vec3<f32>(c) + hash3(c, s));
                if (d < best) { best = d; id = hash1(c, s + 31u); }
            }
        }
    }
    return id;
}

/// Procedural 3D grunge (0..1): 1 clouds, 2 cells, 3 scratches, 4 spots.
fn grunge(p: vec3<f32>, kind: i32, scale: f32, s: u32) -> f32 {
    let q = p / max(scale, 1e-6);
    switch kind {
        case 1: { return fbm01(p, scale, 5, s); }
        case 2: {
            let w = worley(q, s);
            return clamp((w.y - w.x) * 1.5, 0.0, 1.0);
        }
        case 3: {
            // Thin streaks: stretched noise thresholded around its mid value.
            let n = fbm01(vec3<f32>(p.x * 0.15, p.y, p.z * 0.15), scale * 0.3, 3, s);
            return 1.0 - smoothstep(0.0, 0.04, abs(n - 0.5));
        }
        case 4: {
            let w = worley(q * 1.7, s + 5u);
            return 1.0 - smoothstep(0.15, 0.45, w.x);
        }
        default: { return 1.0; }
    }
}

// ---------------------------------------------------------------- shaping

fn smoothstep_soft(edge: f32, width: f32, x: f32) -> f32 {
    return smoothstep(edge - width * 0.5, edge + width * 0.5 + 1e-5, x);
}

/// Balance (-1..1) moves the midpoint, contrast (0..1) narrows the transition (spec §3).
fn balance_contrast(v: f32, balance: f32, contrast: f32) -> f32 {
    let mid = clamp(0.5 - balance * 0.5, 0.0, 1.0);
    let width = mix(1.0, 0.02, clamp(contrast, 0.0, 1.0));
    return clamp((v - mid) / width + 0.5, 0.0, 1.0);
}

// ---------------------------------------------------------------- neighborhood helpers

/// Gaussian blur of the input in world units (seam-aware: skips texels of other islands
/// by testing world distance).
fn blur_world(c: Ctx, radius_world: f32) -> vec4<f32> {
    if (radius_world <= 0.0) { return c.src; }
    let fr = basis(c.n);
    var sum = vec4<f32>(0.0);
    var wsum = 0.0;
    let rings = 6;
    for (var r = 0; r <= rings; r = r + 1) {
        let rr = f32(r) / f32(rings) * radius_world;
        let count = select(1, r * 6, r > 0);
        for (var k = 0; k < count; k = k + 1) {
            let a = (f32(k) + 0.5 * f32(r & 1)) / f32(count) * 2.0 * PI;
            let o = (fr[0] * cos(a) + fr[1] * sin(a)) * rr;
            // Seam-aware: continues across UV seams in seamless mode.
            let t = surface_px(c.xy, c.p, o);
            if (t.z < 0.5) { continue; }
            let w = exp(-2.0 * (rr / radius_world) * (rr / radius_world));
            sum = sum + sample_src_px(t.xy) * w;
            wsum = wsum + w;
        }
    }
    return select(c.src, sum / wsum, wsum > 0.0);
}

// ---------------------------------------------------------------- blending (linear)

fn hard_light1(b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    return select(1.0 - 2.0 * (1.0 - b) * (1.0 - s), 2.0 * b * s, s <= vec3<f32>(0.5));
}

fn soft_light1(b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    let lo = 2.0 * b * s + b * b * (1.0 - 2.0 * s);
    let hi = 2.0 * b * (1.0 - s) + sqrt(max(b, vec3<f32>(0.0))) * (2.0 * s - 1.0);
    return select(hi, lo, s <= vec3<f32>(0.5));
}

fn lum601(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.3, 0.59, 0.11)); }

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    var o = c + (l - lum601(c));
    let lo = lum601(o);
    let n = min(o.r, min(o.g, o.b));
    let x = max(o.r, max(o.g, o.b));
    if (n < 0.0) { o = lo + (o - lo) * lo / max(lo - n, 1e-5); }
    if (x > 1.0) { o = lo + (o - lo) * (1.0 - lo) / max(x - lo, 1e-5); }
    return o;
}

fn sat3(c: vec3<f32>) -> f32 { return max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b)); }

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    if (mx - mn < 1e-5) { return vec3<f32>(0.0); }
    return (c - mn) * s / (mx - mn);
}

/// Reoriented normal mapping: detail `s` on base `b`, both encoded 0..1.
fn rnm(b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    let t = b * vec3<f32>(2.0, 2.0, 2.0) + vec3<f32>(-1.0, -1.0, 0.0);
    let u = s * vec3<f32>(-2.0, -2.0, 2.0) + vec3<f32>(1.0, 1.0, -1.0);
    let r = t * dot(t, u) - u * t.z;
    // Zero base (no normal to rotate): keep `s` instead of normalize(0) = NaN.
    if (dot(r, r) < 1e-12) { return s; }
    return normalize(r) * 0.5 + 0.5;
}

/// Blend modes; ids match `BlendMode::gpu_id`.
fn blend_mode(b: vec3<f32>, s: vec3<f32>, mode: u32) -> vec3<f32> {
    switch mode {
        case 3u: { return b * s; }                                                      // Multiply
        case 4u: { return b + s - b * s; }                                              // Screen
        case 5u: { return hard_light1(s, b); }                                          // Overlay
        case 6u: { return soft_light1(b, s); }                                          // Soft Light
        case 7u: { return hard_light1(b, s); }                                          // Hard Light
        case 8u: { return b + 2.0 * s - 1.0; }                                          // Linear Light
        case 9u: { return b + s; }                                                      // Linear Dodge
        case 10u: { return b - s; }                                                     // Subtract
        case 11u: { return abs(b - s); }                                                // Difference
        case 12u: { return b / max(s, vec3<f32>(1e-4)); }                               // Divide
        case 13u: { return min(b, s); }                                                 // Darken
        case 14u: { return max(b, s); }                                                 // Lighten
        case 15u: { return set_lum(s, lum601(b)); }                                     // Color
        case 16u: { return set_lum(set_sat(s, sat3(b)), lum601(b)); }                   // Hue
        case 17u: { return set_lum(set_sat(b, sat3(s)), lum601(b)); }                   // Saturation
        case 18u: {                                                                     // Value
            let hb = rgb_to_hsv(b);
            return hsv_to_rgb(vec3<f32>(hb.x, hb.y, max(s.r, max(s.g, s.b))));
        }
        case 19u: { return rnm(b, s); }                                                 // Normal Combine
        default: { return s; }                                                          // Normal / Replace / Passthrough
    }
}

/// Straight-alpha composite of `top` (coverage `a`) over `base` (W3C general formula).
fn composite(base: vec4<f32>, top: vec3<f32>, a: f32, mode: u32) -> vec4<f32> {
    let ab = base.a;
    if (a <= 0.0) { return base; }
    var bl = blend_mode(base.rgb, top, mode);
    if (!is_color_channel()) { bl = vec3<f32>(bl.r); }
    let ao = a + ab * (1.0 - a);
    let rgb = ((1.0 - a) * ab * base.rgb + (1.0 - ab) * a * top + a * ab * bl) / max(ao, 1e-6);
    return vec4<f32>(rgb, ao);
}
