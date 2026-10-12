// Shared generator post-processing (spec §3): grunge, balance, contrast, invert.
// Appended after the generated accessors for generator / wear effects only.

fn generator_finish(c: Ctx, v_in: f32) -> f32 {
    var v = clamp(v_in, 0.0, 1.0);
    let gk = p_grunge_type();
    if (gk > 0) {
        let g = grunge(c.p, gk, p_grunge_scale() * radius(), seed() + 101u);
        let gc = balance_contrast(g, 0.0, p_grunge_contrast());
        v = v * mix(1.0, gc, p_grunge_amount());
    }
    v = balance_contrast(v, p_g_balance(), p_g_contrast());
    if (p_g_invert()) { v = 1.0 - v; }
    return v;
}
