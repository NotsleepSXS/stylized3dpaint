// Wood Edge Wear (spec §4): worn edges with splinters stretched along the grain.
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
                let ixy = vec2<i32>(round(vec2<f32>(xy) + to_texels(j, o)));
                if (!covered(ixy)) { continue; }
                if (distance(world(ixy), p) > rr * 2.0 + tw * 2.0) { continue; }
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
    let ixy = vec2<i32>(round(vec2<f32>(c.xy) + to_texels(jacobian(c.xy), normalize(up) * stp)));
    var side = 1.0;
    if (covered(ixy) && distance(world(ixy), c.p) < stp * 3.0 + tw * 2.0) {
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

// ---- effect ----
fn wear_src(xy: vec2<i32>) -> f32 { return wr_edge_std(xy); }
fn wear_width_mul() -> f32 { return 1.0; }
// Splinters: chip detail stretched along the grain direction.
fn wear_detail(p: vec3<f32>, hp: bool) -> f32 {
    let cs = p_chip_size() * radius();
    let iso = wr_chips(p, cs, hp, seed() + 451u);
    let g = normalize(p_grain_dir() + vec3<f32>(0.0, 1e-6, 0.0));
    let along = dot(p, g);
    let perp = p - g * along;
    let stretch = max(p_splinter_length() * radius(), cs) / max(cs, 1e-6);
    let q = perp + g * (along / stretch);
    var sp = 0.0;
    if (hp) {
        sp = mix(cell_id(q / max(cs, 1e-6), seed() + 455u), fbm01(q, cs * 1.5, 1, seed() + 457u), 0.4);
    } else {
        sp = mix(fbm01(q, cs, 4, seed() + 457u), cell_id(q / max(cs, 1e-6), seed() + 455u), 0.35 * p_chip_random());
    }
    return mix(iso, sp, p_splinter_amount());
}

fn effect(c: Ctx) -> vec4<f32> {
    let v = wr_value(c.xy, c.p, c.n);
    let wear = wr_shape(v);
    if (is_mask()) { return vec4<f32>(wear); }
    switch channel() {
        case 0u: {
            // Exposed fresh wood: lighter, towards the raw wood color.
            let l = p_edge_lightening();
            let col = mix(c.src.rgb * (1.0 + 1.5 * l), p_raw_wood(), l);
            return wr_rim_color(c, v, col, wear);
        }
        case 1u: { return vec4<f32>(vec3<f32>(p_raw_roughness()), wear); }
        case 4u: {
            if (p_hand_painted()) { return vec4<f32>(c.src.rgb, wear); }
            return vec4<f32>(vec3<f32>(c.src.r - 0.1 * p_depth() * wear), wear);
        }
        default: { return vec4<f32>(c.src.rgb, wear); }
    }
}
