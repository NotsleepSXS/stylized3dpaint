// Gradient map (spec §5 / §6.5): source value -> up to 5 color stops (interpolated in sRGB).

fn gm_color(i: i32) -> vec3<f32> {
    switch i {
        case 0: { return p_c1(); }
        case 1: { return p_c2(); }
        case 2: { return p_c3(); }
        case 3: { return p_c4(); }
        default: { return p_c5(); }
    }
}

fn gm_pos(i: i32) -> f32 {
    switch i {
        case 0: { return p_t1(); }
        case 1: { return p_t2(); }
        case 2: { return p_t3(); }
        case 3: { return p_t4(); }
        default: { return p_t5(); }
    }
}

fn gm_eval(x: f32) -> vec3<f32> {
    let n = clamp(p_count(), 2, 5);
    if (x <= gm_pos(0)) { return linear_to_srgb3(gm_color(0)); }
    var out = linear_to_srgb3(gm_color(n - 1));
    for (var i = 0; i < n - 1; i = i + 1) {
        let a = gm_pos(i);
        let b = max(gm_pos(i + 1), a);
        if (x <= b) {
            var t = select((x - a) / (b - a), 1.0, b - a < 1e-6);
            switch p_interp() {
                case 1: { t = t * t * (3.0 - 2.0 * t); }
                case 2: { t = 0.0; }
                default: {}
            }
            return mix(linear_to_srgb3(gm_color(i)), linear_to_srgb3(gm_color(i + 1)), t);
        }
    }
    return out;
}

fn effect(c: Ctx) -> vec4<f32> {
    // Base Color (and masks) only: an effect layer must not paint emissive / roughness.
    if (!is_mask() && channel() != 0u) { return c.src; }
    var x = 0.0;
    switch p_source() {
        case 1: { x = c.src.r; }
        case 2: { x = position01(c.p).y; }
        case 3: { x = ao(c.xy); }
        case 4: { x = curvature(c.xy); }
        default: {
            // Display-referred luminance so mid gray sits mid gradient.
            x = select(c.src.r, luminance(c.src.rgb), is_color_channel());
            x = linear_to_srgb3(vec3<f32>(x)).x;
        }
    }
    if (p_source() == 1) { x = linear_to_srgb3(vec3<f32>(x)).x; }
    let g = srgb_to_linear3(gm_eval(clamp(x, 0.0, 1.0)));
    if (is_mask()) {
        return vec4<f32>(vec3<f32>(luminance(g)), c.src.a);
    }
    let keep = clamp(p_preserve_lum(), 0.0, 1.0);
    let lk = max(set_lum(g, lum601(c.src.rgb)), vec3<f32>(0.0));
    return vec4<f32>(mix(g, lk, keep), c.src.a);
}
