// Layer stack compositing. The accumulator is always opaque (it starts from the
// document defaults), so each layer is: out = mix(dst, blend(dst, src), coverage).
// Like Photoshop, blending works on sRGB-encoded values.
// Blend formulas follow PhotoCraft (MIT OR Apache-2.0, github.com/storytold/photocraft).

struct BlendParams {
    // rgb: fill value (fill layers)
    fill: vec4<f32>,
    // x: opacity, y: blend mode, z: source kind (0 color tex, 1 scalar tex, 2 fill), w: use mask
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> b: BlendParams;
@group(0) @binding(1) var dst_tex: texture_2d<f32>;
@group(0) @binding(2) var src_tex: texture_2d<f32>;
@group(0) @binding(3) var mask_tex: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

fn lum(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.3, 0.59, 0.11));
}

fn clip_color(c: vec3<f32>) -> vec3<f32> {
    let l = lum(c);
    let n = min(c.r, min(c.g, c.b));
    let x = max(c.r, max(c.g, c.b));
    var o = c;
    if (n < 0.0) { o = l + (o - l) * l / max(l - n, 1e-5); }
    if (x > 1.0) { o = l + (o - l) * (1.0 - l) / max(x - l, 1e-5); }
    return o;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    return clip_color(c + (l - lum(c)));
}

fn sat(c: vec3<f32>) -> f32 {
    return max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b));
}

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    if (mx - mn < 1e-5) { return vec3<f32>(0.0); }
    return (c - mn) * s / (mx - mn);
}

// Photoshop's soft light (sqrt variant, not the W3C one).
fn soft_light(d: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    let lo = 2.0 * d * s + d * d * (1.0 - 2.0 * s);
    let hi = 2.0 * d * (1.0 - s) + sqrt(max(d, vec3<f32>(0.0))) * (2.0 * s - 1.0);
    return select(hi, lo, s <= vec3<f32>(0.5));
}

fn hard_light(d: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    return select(1.0 - 2.0 * (1.0 - d) * (1.0 - s), 2.0 * d * s, s <= vec3<f32>(0.5));
}

fn blend(d: vec3<f32>, s: vec3<f32>, mode: u32) -> vec3<f32> {
    switch mode {
        case 1u: { return min(d, s); }                                   // Darken
        case 2u: { return d * s; }                                       // Multiply
        case 3u: {                                                       // Color Burn
            return select(1.0 - min(vec3<f32>(1.0), (1.0 - d) / max(s, vec3<f32>(1e-5))), vec3<f32>(1.0), d >= vec3<f32>(1.0));
        }
        case 4u: { return max(d + s - 1.0, vec3<f32>(0.0)); }            // Linear Burn
        case 5u: { return max(d, s); }                                   // Lighten
        case 6u: { return d + s - d * s; }                               // Screen
        case 7u: {                                                       // Color Dodge
            return select(min(vec3<f32>(1.0), d / max(1.0 - s, vec3<f32>(1e-5))), vec3<f32>(0.0), d <= vec3<f32>(0.0));
        }
        case 8u: { return min(d + s, vec3<f32>(1.0)); }                  // Linear Dodge
        case 9u: { return hard_light(s, d); }                            // Overlay
        case 10u: { return soft_light(d, s); }                           // Soft Light
        case 11u: { return hard_light(d, s); }                           // Hard Light
        case 12u: { return abs(d - s); }                                 // Difference
        case 13u: { return max(d - s, vec3<f32>(0.0)); }                 // Subtract
        case 14u: { return set_lum(set_sat(s, sat(d)), lum(d)); }        // Hue
        case 15u: { return set_lum(set_sat(d, sat(s)), lum(d)); }        // Saturation
        case 16u: { return set_lum(s, lum(d)); }                         // Color
        case 17u: { return set_lum(d, lum(s)); }                         // Luminosity
        default: { return s; }                                           // Normal
    }
}

@fragment
fn fs_blend(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pos.xy);
    let d = textureLoad(dst_tex, xy, 0).rgb;
    let kind = u32(b.params.z);
    var s = b.fill.rgb;
    var a = 1.0;
    if (kind == 0u) {
        let t = textureLoad(src_tex, xy, 0);
        a = t.a;
        s = t.rgb / max(a, 1e-4);
    } else if (kind == 1u) {
        let t = textureLoad(src_tex, xy, 0);
        a = t.g;
        s = vec3<f32>(t.r / max(a, 1e-4));
    }
    if (b.params.w > 0.5) {
        a = a * textureLoad(mask_tex, xy, 0).r;
    }
    let k = clamp(a * b.params.x, 0.0, 1.0);
    if (k <= 0.0) { return vec4<f32>(d, 1.0); }
    let blended = clamp(blend(d, clamp(s, vec3<f32>(0.0), vec3<f32>(1.0)), u32(b.params.y)), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(mix(d, blended, k), 1.0);
}
