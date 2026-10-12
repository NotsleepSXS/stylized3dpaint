// Stack compositing passes (linear, straight alpha).
//
// fs_layer:     acc (rgba16f) + layer content (texture straight / premultiplied, or a constant)
//               * mask * opacity, blended with the layer's mode (W3C general formula).
// fs_mask_item: mask (r) + painted item (rg premultiplied: value*a, a) or a constant fill.
// fs_display:   final acc -> display / export texture, extending island borders.

struct LayerParams {
    // rgb: constant value (kind 2)
    value: vec4<f32>,
    // x: mode, y: opacity, z: top kind (0 straight tex, 1 premultiplied tex, 2 constant), w: use mask
    p: vec4<f32>,
    // x: channel is color (0/1)
    q: vec4<f32>,
};

@group(0) @binding(0) var<uniform> L: LayerParams;
@group(0) @binding(1) var acc: texture_2d<f32>;
@group(0) @binding(2) var top_tex: texture_2d<f32>;
@group(0) @binding(3) var mask_tex: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

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
fn rnm(b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    let t = b * vec3<f32>(2.0, 2.0, 2.0) + vec3<f32>(-1.0, -1.0, 0.0);
    let u = s * vec3<f32>(-2.0, -2.0, 2.0) + vec3<f32>(1.0, 1.0, -1.0);
    let r = t * dot(t, u) - u * t.z;
    // A zero base (e.g. the transparent start of a folder) has no normal to rotate: keep `s`.
    if (dot(r, r) < 1e-12) { return s; }
    return normalize(r) * 0.5 + 0.5;
}

// Ids match BlendMode::gpu_id.
fn blend_mode(b: vec3<f32>, s: vec3<f32>, mode: u32) -> vec3<f32> {
    switch mode {
        case 3u: { return b * s; }
        case 4u: { return b + s - b * s; }
        case 5u: { return hard_light1(s, b); }
        case 6u: { return soft_light1(b, s); }
        case 7u: { return hard_light1(b, s); }
        case 8u: { return b + 2.0 * s - 1.0; }
        case 9u: { return b + s; }
        case 10u: { return b - s; }
        case 11u: { return abs(b - s); }
        case 12u: { return b / max(s, vec3<f32>(1e-4)); }
        case 13u: { return min(b, s); }
        case 14u: { return max(b, s); }
        case 15u: { return set_lum(s, lum601(b)); }
        case 16u: { return set_lum(set_sat(s, sat3(b)), lum601(b)); }
        case 17u: { return set_lum(set_sat(b, sat3(s)), lum601(b)); }
        case 18u: {
            let hb = rgb_to_hsv(b);
            return hsv_to_rgb(vec3<f32>(hb.x, hb.y, max(s.r, max(s.g, s.b))));
        }
        case 19u: { return rnm(b, s); }
        default: { return s; }
    }
}

@fragment
fn fs_layer(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pos.xy);
    let base = textureLoad(acc, xy, 0);
    let kind = u32(L.p.z);
    var top = L.value;
    if (kind == 0u) {
        top = textureLoad(top_tex, xy, 0);
    } else if (kind == 1u) {
        let t = textureLoad(top_tex, xy, 0);
        top = vec4<f32>(t.rgb / max(t.a, 1e-5), t.a);
    }
    var a = top.a * L.p.y;
    if (L.p.w > 0.5) { a = a * textureLoad(mask_tex, xy, 0).r; }
    a = clamp(a, 0.0, 1.0);
    if (a <= 0.0) { return base; }
    // Nothing below yet (a folder starts transparent): the layer lands as is. Blend modes are
    // not evaluated against the empty base (RNM of a zero normal is NaN, and 0 * NaN = NaN).
    if (base.a <= 0.0) { return vec4<f32>(top.rgb, a); }
    let mode = u32(L.p.x);
    var bl = blend_mode(base.rgb, top.rgb, mode);
    if (L.q.x < 0.5) { bl = vec3<f32>(bl.r); }
    let ab = base.a;
    let ao = a + ab * (1.0 - a);
    let rgb = ((1.0 - a) * ab * base.rgb + (1.0 - ab) * a * top.rgb + a * ab * bl) / max(ao, 1e-6);
    return vec4<f32>(rgb, ao);
}

@fragment
fn fs_mask_item(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pos.xy);
    let base = textureLoad(acc, xy, 0).r;
    var v = L.value.r;
    var a = 1.0;
    if (u32(L.p.z) == 1u) {
        let t = textureLoad(top_tex, xy, 0);
        a = t.g;
        v = t.r / max(t.g, 1e-5);
    }
    a = clamp(a * L.p.y, 0.0, 1.0);
    let b = blend_mode(vec3<f32>(base), vec3<f32>(v), u32(L.p.x)).r;
    return vec4<f32>(clamp(mix(base, b, a), 0.0, 1.0));
}

// Content source: premultiplied paint texture or a constant -> straight rgba buffer.
@fragment
fn fs_prep(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let xy = vec2<i32>(pos.xy);
    if (u32(L.p.z) == 2u) { return L.value; }
    let t = textureLoad(top_tex, xy, 0);
    if (u32(L.p.z) == 1u) { return vec4<f32>(t.rgb / max(t.a, 1e-5), t.a); }
    return t;
}
