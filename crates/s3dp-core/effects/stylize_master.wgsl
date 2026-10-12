// Main stylization filter (spec §6.1). Groups run in order:
// smoothness -> brush strokes -> colorize -> gradient -> baked lighting -> edges & cavities.
// Everything spatial is evaluated in world space (seamless, resolution independent).
// Only Smoothness touches every channel; Edges & Cavities also nudges roughness; the rest
// changes Base Color only.

// ---------------------------------------------------------------- brush strokes

// Tangent reference direction: world up projected on the surface (X near the poles).
fn sm_ref_dir(n: vec3<f32>) -> vec3<f32> {
    let k = smoothstep(0.8, 0.98, abs(n.y));
    let r = normalize(mix(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), k));
    let t = r - n * dot(r, n);
    let len = length(t);
    if (len < 1e-5) { return basis(n)[0]; }
    return t / len;
}

fn sm_rotate(v: vec3<f32>, axis: vec3<f32>, a: f32) -> vec3<f32> {
    return v * cos(a) + cross(axis, v) * sin(a) + axis * dot(axis, v) * (1.0 - cos(a));
}

struct StrokeOut {
    // weighted stroke color (rgb) and total weight (a)
    col: vec4<f32>,
    // weighted value / hue jitter (x, y), max coverage (z)
    jit: vec3<f32>,
};

fn sm_strokes(c: Ctx) -> StrokeOut {
    var o: StrokeOut;
    o.col = vec4<f32>(0.0);
    o.jit = vec3<f32>(0.0);
    let g = max(p_br_scale() * radius(), 1e-5);
    let q = c.p / g;
    let ci = vec3<i32>(floor(q));
    let j = jacobian(c.xy);
    let tw = texel_world(c.xy);
    let ct = cos(radians(p_br_normal_thr()));
    let s = seed() * 7u + 101u;
    for (var z = -1; z <= 1; z = z + 1) {
        for (var y = -1; y <= 1; y = y + 1) {
            for (var x = -1; x <= 1; x = x + 1) {
                let cell = ci + vec3<i32>(x, y, z);
                let h0 = hash3(cell, s);
                let h1 = hash3(cell, s + 17u);
                if (h1.x > p_br_amount()) { continue; }
                let center = (vec3<f32>(cell) + h0) * g;
                // Stroke anchored on the surface: project the cell point on the tangent plane.
                let hgt = dot(center - c.p, c.n);
                if (abs(hgt) > g) { continue; }
                let cs = center - c.n * hgt;
                // Normal at the stroke center (falls back to ours when it maps off the island).
                var ns = c.n;
                let ct_px = surface_px(c.xy, c.p, cs - c.p);
                let cxy = vec2<i32>(round(ct_px.xy));
                if (ct_px.z > 0.5 && distance(world(cxy), cs) < g * 0.5 + tw * 2.0) { ns = world_normal(cxy); }
                let facing = dot(ns, c.n);
                let nf = smoothstep(ct - 0.08, ct + 0.02, facing);
                if (nf <= 0.0) { continue; }
                let sz = mix(1.0, 0.5 + h1.y, p_br_scale_rand());
                let hl = clamp(p_br_size_x() * sz * 0.62, 0.05, 1.0) * g;
                let hw = clamp(p_br_size_y() * sz * 0.62, 0.03, 1.0) * g;
                let ang = radians(p_br_rotation()) + (h1.z - 0.5) * 2.0 * PI * p_br_rot_rand();
                let d = sm_rotate(sm_ref_dir(ns), ns, ang);
                let e = cross(ns, d);
                let off = c.p - cs;
                let a = dot(off, d) / hl;
                let b = dot(off, e) / hw;
                // Tapered ellipse: rounder head, thinner tail.
                let taper = 1.0 - 0.35 * clamp(a, -1.0, 1.0) * clamp(a, -1.0, 1.0) - 0.15 * a;
                let dd = a * a + (b / max(taper, 0.2)) * (b / max(taper, 0.2));
                let m = (1.0 - smoothstep(0.65, 1.0, dd)) * nf;
                if (m <= 0.001) { continue; }
                // Average the input along the stroke axis (flat painterly color).
                var sum = vec3<f32>(0.0);
                var cnt = 0.0;
                for (var k = -2; k <= 2; k = k + 1) {
                    let wp = cs + d * (f32(k) * 0.4 * hl);
                    let tap = surface_px(c.xy, c.p, wp - c.p);
                    if (tap.z < 0.5) { continue; }
                    let ixy = vec2<i32>(round(tap.xy));
                    if (distance(world(ixy), wp) > hl * 0.5 + tw * 2.0) { continue; }
                    sum = sum + sample_src_px(tap.xy).rgb;
                    cnt = cnt + 1.0;
                }
                if (cnt < 0.5) { continue; }
                let w = m * exp(6.0 * h0.x * h1.x + 6.0 * hash1(cell, s + 3u));
                // Bristle streaks along the stroke.
                let bristle = gnoise(vec3<f32>(b * 3.0, h0.y * 37.0, h0.z * 11.0), s + 9u);
                o.col = o.col + vec4<f32>(sum / cnt * w, w);
                o.jit = vec3<f32>(o.jit.xy + vec2<f32>((h0.y - 0.5) * 0.3 + bristle * 0.08, (h0.z - 0.5) * 0.06) * w, max(o.jit.z, m));
            }
        }
    }
    return o;
}

// ---------------------------------------------------------------- shared helpers

fn sm_blend_id(i: i32) -> u32 {
    switch i {
        case 1: { return 3u; }
        case 2: { return 4u; }
        case 3: { return 5u; }
        case 4: { return 6u; }
        default: { return 0u; }
    }
}

// x: convex (edges), y: concave (cavities), averaged over a small world disc.
fn sm_edge_cav(c: Ctx) -> vec2<f32> {
    let rad = p_ec_spread() * radius();
    var acc = vec2<f32>(0.0);
    let j = jacobian(c.xy);
    let fr = basis(c.n);
    let tw = texel_world(c.xy);
    let rings = select(0, 2, rad > 0.0);
    for (var r = 0; r <= rings; r = r + 1) {
        let rr = select(0.0, f32(r) / f32(max(rings, 1)) * rad, rings > 0);
        let count = select(1, r * 6, r > 0);
        for (var k = 0; k < count; k = k + 1) {
            let a = (f32(k) + 0.5 * f32(r & 1)) / f32(count) * 2.0 * PI;
            let tap = surface_px(c.xy, c.p, (fr[0] * cos(a) + fr[1] * sin(a)) * rr);
            if (tap.z < 0.5) { continue; }
            let ixy = vec2<i32>(round(tap.xy));
            let kv = curvature(ixy);
            let v = vec2<f32>(max(edges(ixy), clamp((kv - 0.5) * 2.0, 0.0, 1.0)), max(cavity(ixy), clamp((0.5 - kv) * 2.0, 0.0, 1.0)));
            // Max-ish accumulation so spread widens instead of fading.
            acc = max(acc, v * (1.0 - 0.5 * rr / max(rad, 1e-6)));
        }
    }
    return acc;
}

fn sm_contrast(v: f32, k: f32) -> f32 { return smoothstep(0.25 * k, 1.0 - 0.6 * k, v); }

// ---------------------------------------------------------------- main

fn effect(c: Ctx) -> vec4<f32> {
    let gs = clamp(p_strength(), 0.0, 1.0);
    var v = c.src;
    // Smoothness (every channel and masks).
    let st_sm = gs * p_sm_strength();
    if (st_sm > 0.0) {
        v = mix(v, blur_world(c, p_sm_radius() * radius()), st_sm);
    }
    if (is_mask()) { return v; }
    let ch = channel();
    let ec_on = gs * p_ec_strength();
    if (ch == 1u && ec_on > 0.0) {
        // Roughness: worn edges a little smoother, cavities a little rougher.
        let ek = sm_edge_cav(c);
        let mode = p_ec_mode();
        let e = select(0.0, sm_contrast(ek.x, p_ec_edge_contrast()) * p_ec_edge_op(), mode != 1);
        let k = select(0.0, sm_contrast(ek.y, p_ec_cav_contrast()) * p_ec_cav_op(), mode != 0);
        let r = clamp(v.r - 0.1 * e * ec_on + 0.08 * k * ec_on, 0.0, 1.0);
        return vec4<f32>(vec3<f32>(r), v.a);
    }
    if (ch != 0u) { return v; }
    var col = v.rgb;

    // Brush strokes.
    let st_br = gs * p_br_strength();
    if (st_br > 0.0) {
        let so = sm_strokes(c);
        if (so.col.a > 0.0) {
            let mean = so.col.rgb / so.col.a;
            let jit = so.jit.xy / so.col.a * p_br_color_var();
            var hsv = rgb_to_hsv(linear_to_srgb3(max(mean, vec3<f32>(0.0))));
            hsv.x = fract(hsv.x + jit.y + 1.0);
            hsv.z = clamp(hsv.z * (1.0 + jit.x), 0.0, 1.0);
            let painted = srgb_to_linear3(hsv_to_rgb(hsv));
            col = mix(col, painted, clamp(so.jit.z, 0.0, 1.0) * st_br);
        }
    }

    // Colorize: Color blend of the tint, amount broken up by a 3D grunge.
    let st_co = gs * p_co_strength();
    if (st_co > 0.0) {
        let gr = fbm01(c.p, p_co_scale() * radius(), 4, seed() + 11u);
        let amt = st_co * mix(1.0, smoothstep(0.3, 0.7, gr), p_co_grunge());
        col = mix(col, max(set_lum(p_co_color(), lum601(col)), vec3<f32>(0.0)), amt);
    }

    // Gradient in object space.
    let st_gr = gs * p_gr_strength();
    if (st_gr > 0.0) {
        let h = radians(p_gr_h());
        let e = radians(p_gr_v());
        let dir = vec3<f32>(cos(e) * cos(h), sin(e), cos(e) * sin(h));
        let center = U.bbox_min.xyz + U.bbox_size.xyz * 0.5;
        let ext = max(dot(abs(dir), U.bbox_size.xyz) * 0.5, 1e-6);
        var t = dot(c.p - center, dir) / ext * 0.5 + 0.5 + p_gr_offset();
        t = balance_contrast(clamp(t, 0.0, 1.0), 0.0, p_gr_contrast());
        if (p_gr_invert()) { t = 1.0 - t; }
        let c1 = blend_mode(col, p_gr_color1(), sm_blend_id(p_gr_blend1()));
        col = mix(col, max(c1, vec3<f32>(0.0)), t * p_gr_op1() * st_gr);
        let c2 = blend_mode(col, p_gr_color2(), sm_blend_id(p_gr_blend2()));
        col = mix(col, max(c2, vec3<f32>(0.0)), (1.0 - t) * p_gr_op2() * st_gr);
    }

    // Baked lighting: wrapped diffuse with tinted light side, darkened shadow side, AO / sky
    // occlusion and a soft view-independent highlight.
    let st_bl = gs * p_bl_strength();
    if (st_bl > 0.0) {
        let l = normalize(p_bl_dir() + vec3<f32>(0.0, 1e-6, 0.0));
        let ndl = dot(c.n, l);
        let wrap = ndl * 0.5 + 0.5;
        let soft = p_bl_diff_radius() * 0.5 + 0.03;
        let lit = smoothstep(0.5 - soft, 0.5 + soft, wrap);
        let kc = p_bl_diff_contrast();
        let occl = ao(c.xy) * mix(1.0, sky(c.xy), 0.5);
        var light = mix(vec3<f32>(1.0 - 0.65 * kc), p_bl_diff_color() * (1.0 + 0.35 * kc), lit);
        light = light * mix(1.0, occl, 0.6);
        light = mix(vec3<f32>(1.0), light, p_bl_diff_int());
        var lc = col * light;
        let expo = mix(96.0, 3.0, p_bl_spec_radius());
        let sp = pow(max(ndl, 0.0), expo) * p_bl_spec_int() * occl;
        lc = lc + p_bl_spec_color() * sp * 0.6;
        col = mix(col, lc, st_bl);
    }

    // Edges & cavities.
    if (ec_on > 0.0) {
        let ek = sm_edge_cav(c);
        let mode = p_ec_mode();
        if (mode != 1) {
            let e = sm_contrast(ek.x, p_ec_edge_contrast());
            let scr = col + p_ec_edge_color() - col * p_ec_edge_color();
            col = mix(col, scr, e * p_ec_edge_op() * ec_on);
        }
        if (mode != 0) {
            let k = sm_contrast(ek.y, p_ec_cav_contrast());
            col = mix(col, col * p_ec_cav_color(), k * p_ec_cav_op() * ec_on);
        }
    }
    return vec4<f32>(col, v.a);
}
