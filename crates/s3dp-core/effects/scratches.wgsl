// Scratches (spec §4): straight 3D scratch segments inside the wear area.
// Content target ("content_output": "value"): rgb = this channel's value, a = wear amount.
// All distances are relative to radius(); all noise is 3D in world space (no UV seams).

// ---- shared wear core (identical in every spec §4 wear effect) --------------------------------
// Each wear effect defines:
//   fn wear_src(xy) -> f32      the raw "wear here" source (edges, cavities, anchor ...), 0..1
//   fn wear_detail(p, hp) -> f32 chip / flake detail in 0..1 (mean ~0.5), 3D
//   fn wear_width_mul() -> f32  multiplier of Edge Width
// The core grows the source by Edge Width (world units), masks it with AO, location bias and the
// large breakup noise, adds the chip detail and thresholds it by Wear Level / Contrast.

/// Standard convex edge source from the baked maps.
fn wr_edge_std(xy: vec2<i32>) -> f32 {
    let k = clamp((curvature(xy) - 0.5) * 2.0, 0.0, 1.0);
    return smoothstep(0.05, 0.6, max(edges(xy), k));
}

/// Wear source widened by Edge Width: a soft dilation (max with falloff) blended with the mean
/// (Edges Smoothness).
fn wr_edges(xy: vec2<i32>, p: vec3<f32>, n: vec3<f32>) -> f32 {
    let e0 = wear_src(xy);
    let rw = p_edge_width() * radius() * wear_width_mul();
    let j = jacobian(xy);
    let fr = basis(n);
    let tw = texel_world(xy);
    var mx = e0;
    var sum = e0;
    var wsum = 1.0;
    if (rw > tw * 0.5) {
        for (var r = 1; r <= 3; r = r + 1) {
            let t = f32(r) / 3.0;
            let rr = t * rw;
            for (var k = 0; k < 8; k = k + 1) {
                let a = (f32(k) + 0.5 * f32(r & 1)) / 8.0 * 2.0 * PI;
                let o = (fr[0] * cos(a) + fr[1] * sin(a)) * rr;
                let tap = surface_px(xy, p, o);
                if (tap.z < 0.5) { continue; }
                let ixy = vec2<i32>(round(tap.xy));
                let e = wear_src(ixy);
                mx = max(mx, e * (1.0 - t * t * 0.75));
                sum = sum + e;
                wsum = wsum + 1.0;
            }
        }
    }
    let mean = clamp(sum / wsum * 1.6, 0.0, 1.0);
    return mix(mx, mean, p_edge_smooth());
}

fn wr_center() -> vec3<f32> { return U.bbox_min.xyz + U.bbox_size.xyz * 0.5; }

/// Location bias multiplier (0..2): towards a world direction or towards protrusions.
fn wr_location(p: vec3<f32>) -> f32 {
    let s = p_loc_strength();
    if (s <= 0.0) { return 1.0; }
    let o = p - wr_center();
    var t = 0.5;
    if (p_loc_mode() == 1) {
        t = clamp(length(o) / max(radius(), 1e-6), 0.0, 1.0);
        t = t * t;
    } else {
        let d = normalize(p_loc_dir() + vec3<f32>(0.0, 1e-6, 0.0));
        t = clamp(dot(o, d) / max(radius(), 1e-6) * 0.5 + 0.5, 0.0, 1.0);
    }
    return mix(1.0, smoothstep(0.0, 1.0, t) * 2.0, s);
}

/// Large 3D noise that removes wear in patches so edges are not worn evenly (0..1 multiplier).
fn wr_breakup(p: vec3<f32>) -> f32 {
    let b = fbm01(p, p_breakup_scale() * radius(), 3, seed() + 211u);
    let patches = smoothstep(0.38, 0.58, b);
    return mix(1.0, patches, p_breakup());
}

/// Wear field before the chip detail (0..~1).
fn wr_field(xy: vec2<i32>, p: vec3<f32>, n: vec3<f32>) -> f32 {
    let cw = p_curv_weight();
    var e = 0.0;
    if (cw > 0.0) { e = wr_edges(xy, p, n); }
    var g = 0.0;
    if (cw < 1.0) {
        // Grunge-only wear source: soft 3D clouds a few chips wide.
        g = smoothstep(0.3, 0.75, fbm01(p, p_chip_size() * radius() * 5.0, 3, seed() + 301u));
    }
    var w = mix(g, e, cw);
    // Occluded areas are protected from wear.
    w = w * mix(1.0, smoothstep(0.25, 0.85, ao(xy)), p_ao_mask());
    return w * wr_location(p) * wr_breakup(p);
}

/// Value compared against the Wear Level threshold.
fn wr_value(xy: vec2<i32>, p: vec3<f32>, n: vec3<f32>) -> f32 {
    let d = (wear_detail(p, p_hand_painted()) - 0.5) * 2.0;
    return wr_field(xy, p, n) + d * 0.55;
}

fn wr_threshold() -> f32 { return mix(1.05, -0.15, p_level()); }

fn wr_width() -> f32 {
    if (p_hand_painted()) { return 0.012; }
    return mix(0.6, 0.03, p_contrast());
}

/// Thresholded wear amount 0..1.
fn wr_shape(v: f32) -> f32 { return clamp((v - wr_threshold()) / wr_width() + 0.5, 0.0, 1.0); }

/// Hand-painted rim just outside the chip: x = rim amount, y = 1 light (chip below) / 0 dark.
fn wr_rim(c: Ctx, v: f32) -> vec2<f32> {
    let rw = p_rim_width() * 0.25;
    if (!p_hand_painted() || rw <= 0.0) { return vec2<f32>(0.0); }
    let thr = wr_threshold();
    let band = smoothstep(thr - rw - 0.01, thr - rw + 0.01, v) * (1.0 - wr_shape(v));
    if (band <= 0.001) { return vec2<f32>(0.0); }
    // Which side of the chip are we on: compare with a point a bit higher up the surface.
    var up = vec3<f32>(0.0, 1.0, 0.0) - c.n * c.n.y;
    if (length(up) < 0.05) { up = basis(c.n)[1]; }
    let tw = texel_world(c.xy);
    let stp = max(p_chip_size() * radius() * 0.5, tw * 2.0);
    let tap = surface_px(c.xy, c.p, normalize(up) * stp);
    let ixy = vec2<i32>(round(tap.xy));
    var side = 1.0;
    if (tap.z > 0.5) {
        side = select(1.0, 0.0, wr_value(ixy, world(ixy), world_normal(ixy)) > v);
    }
    return vec2<f32>(band, side);
}

/// Applies the hand-painted rim to a base color (returns rgb and the extra coverage).
fn wr_rim_color(c: Ctx, v: f32, rgb: vec3<f32>, a: f32) -> vec4<f32> {
    let r = wr_rim(c, v);
    if (r.x <= 0.0) { return vec4<f32>(rgb, a); }
    let rim = mix(p_rim_dark(), p_rim_light(), r.y);
    // Inside the chip keep the chip color; outside paint the rim over the source.
    let col = select(rim, rgb, a > 0.5);
    return vec4<f32>(col, max(a, r.x));
}
// ---- end of shared wear core ------------------------------------------------------------------

/// Generic chip detail: domain-warped Worley cells (one random level per chip) mixed with fbm for
/// ragged borders; hand-painted mode drops the fine noise for clean flat shapes.
fn wr_chips(p: vec3<f32>, cell: f32, hp: bool, s: u32) -> f32 {
    let q = p / max(cell, 1e-6);
    let rnd = p_chip_random();
    let wq = q * 0.45;
    let warp = vec3<f32>(gnoise(wq, s + 1u), gnoise(wq + vec3<f32>(17.3, 5.1, 9.7), s + 2u), gnoise(wq + vec3<f32>(3.9, 29.2, 13.1), s + 3u));
    let cid = cell_id(q + warp * (0.25 + 0.5 * rnd), s + 5u);
    let cellv = mix(0.5, cid, 0.35 + 0.65 * rnd);
    if (hp) {
        return mix(cellv, fbm01(q, 1.6, 1, s + 7u), 0.45);
    }
    return mix(fbm01(q, 1.0, 4, s + 7u), cellv, 0.45);
}

/// Sparse straight scratch segments on the surface (0..1). `d0` is the base direction in the
/// tangent plane, `arand` the angle spread (0..1 = +-90 deg), `len` / `wid` in world units,
/// `count` the share of active candidates (0..1).
fn sc_lines(p: vec3<f32>, n: vec3<f32>, d0: vec3<f32>, arand: f32, len: f32, wid: f32, count: f32, tw: f32, hp: bool, s: u32) -> f32 {
    if (count <= 0.0 || len <= 0.0) { return 0.0; }
    let q = p / len;
    let i = vec3<i32>(floor(q));
    let t2 = normalize(cross(n, d0));
    // Anti-aliasing: never thinner than ~0.7 texel, fade instead. Hand-painted lines stay
    // solid (no fade) and at least ~1.5 texels wide so they do not alias away.
    let w = select(max(wid, tw * 0.7), max(wid, tw * 1.5), hp);
    let fade_w = select(clamp(wid / w, 0.2, 1.0), 1.0, hp);
    var best = 0.0;
    for (var z = -1; z <= 1; z = z + 1) {
        for (var y = -1; y <= 1; y = y + 1) {
            for (var x = -1; x <= 1; x = x + 1) {
                let cc = i + vec3<i32>(x, y, z);
                for (var k = 0u; k < 3u; k = k + 1u) {
                    let h = hash3(cc, s + k * 131u);
                    if (h.x > count) { continue; }
                    let h2 = hash3(cc, s + k * 131u + 61u);
                    let ctr = (vec3<f32>(cc) + h2) * len;
                    var o = p - ctr;
                    let hn = dot(o, n);
                    if (abs(hn) > len * 0.5) { continue; }
                    o = o - n * hn;
                    let ang = (h.y - 0.5) * PI * arand;
                    let d = d0 * cos(ang) + t2 * sin(ang);
                    let half_len = len * mix(0.35, 0.9, h.z);
                    let along = dot(o, d);
                    if (abs(along) > half_len) { continue; }
                    let perp = abs(dot(o, cross(n, d)));
                    let u = along / half_len;
                    let taper = 1.0 - u * u;
                    let ww = w * mix(0.6, 1.0, h2.x) * select(sqrt(taper), 1.0, hp);
                    var line = 1.0 - smoothstep(ww * 0.4, ww, perp);
                    if (hp) { line = select(0.0, 1.0, perp < ww); }
                    let strength = select(mix(0.45, 1.0, fract(h.x * 13.7 + h2.y)) * taper, 1.0, hp);
                    best = max(best, line * strength * fade_w);
                }
            }
        }
    }
    return best;
}

/// Tangent direction at angle `deg` from the horizontal tangent (world up reference).
fn sc_tangent_dir(n: vec3<f32>, ref_dir: vec3<f32>) -> vec3<f32> {
    var d = ref_dir - n * dot(ref_dir, n);
    if (length(d) < 1e-3) { d = basis(n)[0]; }
    return normalize(d);
}

// ---- effect ----
fn wear_src(xy: vec2<i32>) -> f32 { return wr_edge_std(xy); }
fn wear_width_mul() -> f32 { return 1.0; }
// The wear area for scratches is soft and cloudy; the scratch lines carry the detail.
fn wear_detail(p: vec3<f32>, hp: bool) -> f32 {
    if (hp) { return 0.5; }
    return fbm01(p, p_chip_size() * radius() * 3.0, 2, seed() + 461u);
}

fn effect(c: Ctx) -> vec4<f32> {
    let v = wr_value(c.xy, c.p, c.n);
    let area = wr_shape(v);
    var s = 0.0;
    if (area > 0.0) {
        // Base direction: Angle measured in the tangent plane from the horizontal tangent.
        var hz = cross(vec3<f32>(0.0, 1.0, 0.0), c.n);
        if (length(hz) < 1e-3) { hz = basis(c.n)[0]; }
        hz = normalize(hz);
        let vt = cross(c.n, hz);
        let a = radians(p_angle());
        let d0 = hz * cos(a) + vt * sin(a);
        let len = p_length() * radius();
        let wid = p_width() * radius();
        s = sc_lines(c.p, c.n, d0, p_angle_random(), len, wid, p_count(), texel_world(c.xy), p_hand_painted(), seed() + 467u);
        // Second, finer and sparser set for variety (not in hand-painted mode).
        if (!p_hand_painted()) {
            s = max(s, 0.6 * sc_lines(c.p, c.n, d0, min(p_angle_random() + 0.2, 1.0), len * 0.45, wid * 0.6, p_count() * 0.6, texel_world(c.xy), false, seed() + 479u));
        }
    }
    let sc = s * area;
    if (is_mask()) { return vec4<f32>(sc); }
    switch channel() {
        case 0u: { return vec4<f32>(mix(c.src.rgb, p_scratch_color(), p_color_amount()), sc); }
        case 1u: { return vec4<f32>(vec3<f32>(p_scratch_roughness()), sc); }
        case 4u: {
            if (p_hand_painted()) { return vec4<f32>(c.src.rgb, sc); }
            return vec4<f32>(vec3<f32>(c.src.r - 0.2 * p_depth()), sc);
        }
        default: { return vec4<f32>(c.src.rgb, sc); }
    }
}
