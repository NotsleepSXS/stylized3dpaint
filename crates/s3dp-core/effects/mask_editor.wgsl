// Mask editor: fixed-order combination of map / position / noise layers.

fn me_blend(b: f32, s: f32, mode: i32) -> f32 {
    switch mode {
        case 1: { return b * s; }
        case 2: { return b - s; }
        case 3: { return max(b, s); }
        case 4: { return min(b, s); }
        case 5: { return select(1.0 - 2.0 * (1.0 - b) * (1.0 - s), 2.0 * b * s, b < 0.5); }
        default: { return b + s; }
    }
}

fn me_layer(b: f32, s_in: f32, opacity: f32, mode: i32, inv: bool) -> f32 {
    if (opacity <= 0.0) { return b; }
    let s = select(s_in, 1.0 - s_in, inv);
    return clamp(mix(b, me_blend(b, s, mode), opacity), 0.0, 1.0);
}

fn effect(c: Ctx) -> vec4<f32> {
    var v = 0.0;
    // AO: white = occluded.
    v = me_layer(v, 1.0 - ao(c.xy), p_ao_opacity(), p_ao_blend(), p_ao_invert());
    // Curvature: convex edges.
    let cx = max(edges(c.xy), clamp((curvature(c.xy) - 0.5) * 2.0, 0.0, 1.0));
    v = me_layer(v, cx, p_curv_opacity(), p_curv_blend(), p_curv_invert());
    // Position: Y gradient over the bounding box (white = top).
    let py = balance_contrast(position01(c.p).y, 0.0, p_pos_contrast());
    v = me_layer(v, py, p_pos_opacity(), p_pos_blend(), p_pos_invert());
    // World space normal: facing up.
    let ang = degrees(acos(clamp(c.n.y, -1.0, 1.0)));
    let up = 1.0 - smoothstep(p_wsn_angle() * 0.7, p_wsn_angle() * 1.3 + 0.01, ang);
    v = me_layer(v, up, p_wsn_opacity(), p_wsn_blend(), p_wsn_invert());
    // Thickness: white = thin.
    v = me_layer(v, 1.0 - thickness(c.xy), p_thick_opacity(), p_thick_blend(), p_thick_invert());
    // Noise.
    if (p_noise_opacity() > 0.0) {
        let n = fbm01(c.p, p_noise_scale() * radius(), 4, seed() + 81u);
        v = me_layer(v, n, p_noise_opacity(), p_noise_blend(), p_noise_invert());
    }
    return vec4<f32>(v);
}
