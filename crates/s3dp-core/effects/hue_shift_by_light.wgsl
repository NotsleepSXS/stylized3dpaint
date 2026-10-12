// Hue shift by light (spec §6.5): rotates hue towards a cool goal in shadow and a warm goal in
// light (shortest way around the hue circle), in sRGB HSV.

fn hs_toward(h: f32, goal: f32, t: f32) -> f32 {
    var d = goal - h;
    d = d - round(d);
    return fract(h + d * clamp(t, 0.0, 1.0) + 1.0);
}

fn effect(c: Ctx) -> vec4<f32> {
    if (is_mask() || channel() != 0u) { return c.src; }
    let l = normalize(p_light_dir() + vec3<f32>(0.0, 1e-6, 0.0));
    let wrap = dot(c.n, l) * 0.5 + 0.5;
    let soft = p_softness() * 0.5 + 0.02;
    var lit = smoothstep(0.5 - soft, 0.5 + soft, wrap);
    lit = lit * mix(1.0, ao(c.xy), p_ao_influence());
    let sh = 1.0 - lit;
    var hsv = rgb_to_hsv(linear_to_srgb3(c.src.rgb));
    let gs = rgb_to_hsv(linear_to_srgb3(p_shadow_tint())).x;
    let gl = rgb_to_hsv(linear_to_srgb3(p_light_tint())).x;
    hsv.x = hs_toward(hsv.x, gs, p_shadow_shift() * sh);
    hsv.x = hs_toward(hsv.x, gl, p_light_shift() * lit);
    hsv.y = clamp(hsv.y * (1.0 + p_shadow_sat() * sh + p_light_sat() * lit), 0.0, 1.0);
    hsv.z = clamp(hsv.z * (1.0 + p_value_contrast() * (lit - 0.5) * 0.8), 0.0, 1.0);
    return vec4<f32>(srgb_to_linear3(hsv_to_rgb(hsv)), c.src.a);
}
