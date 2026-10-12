// Edge highlight (spec §6.4): convex edge mask (baked edges + curvature), sampled through a
// small 3D noise warp and eroded by noise for a torn hand-painted edge, then lightens Base Color.

fn eh_edge(xy: vec2<i32>) -> f32 {
    let kv = curvature(xy);
    return max(edges(xy), clamp((kv - 0.5) * 2.0, 0.0, 1.0));
}

fn eh_near_seam(xy: vec2<i32>) -> bool {
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            if (!covered(xy + vec2<i32>(dx, dy))) { return true; }
        }
    }
    return false;
}

fn effect(c: Ctx) -> vec4<f32> {
    if (is_mask() || channel() != 0u) { return c.src; }
    if (p_exclude_seams() && eh_near_seam(c.xy)) { return c.src; }
    let bs = max(p_breakup_scale() * radius(), 1e-5);
    let br = clamp(p_breakup(), 0.0, 1.0);
    var xy = c.xy;
    if (br > 0.0) {
        // Warp the lookup along the surface: wobbly edge outline.
        let fr = basis(c.n);
        let wv = vec2<f32>(fbm01(c.p, bs, 3, seed()) - 0.5, fbm01(c.p, bs, 3, seed() + 19u) - 0.5) * 2.0;
        let o = (fr[0] * wv.x + fr[1] * wv.y) * bs * 0.35 * br;
        let tap = surface_px(c.xy, c.p, o);
        if (tap.z > 0.5) { xy = vec2<i32>(round(tap.xy)); }
    }
    var e = eh_edge(xy);
    if (br > 0.0) {
        // Torn erosion (multiplicative: flat areas never light up).
        let n2 = fbm01(c.p, bs * 0.5, 4, seed() + 41u);
        e = e * clamp(1.0 - br * (1.0 - smoothstep(0.25, 0.65, n2)) * 1.2, 0.0, 1.0);
    }
    let w = clamp(p_width(), 0.0, 0.95);
    let soft = p_softness() * 0.5 + 0.02;
    let th = 1.0 - w;
    var k = smoothstep(th - soft * 0.5, th + soft * 0.5, e);
    if (p_only_top()) { k = k * smoothstep(0.05, 0.45, c.n.y); }
    if (k <= 0.0) { return c.src; }
    var hi = p_color();
    if (p_mode() == 1) {
        let s = linear_to_srgb3(c.src.rgb);
        hi = srgb_to_linear3(mix(s, vec3<f32>(1.0), p_lighter() * 0.01));
    }
    // Highlights never darken.
    hi = max(hi, c.src.rgb);
    return vec4<f32>(mix(c.src.rgb, hi, k), c.src.a);
}
