// Posterize: quantize display-referred values with soft steps.

fn pz_q(x: f32, n: f32, soft: f32) -> f32 {
    let y = clamp(x, 0.0, 1.0) * (n - 1.0);
    let i = floor(y);
    let f = y - i;
    let s = max(soft, 0.001) * 0.5;
    return (i + smoothstep(0.5 - s, 0.5 + s, f)) / (n - 1.0);
}

fn effect(c: Ctx) -> vec4<f32> {
    let ch = channel();
    if (!is_mask() && ch == 3u) { return c.src; }
    let n = f32(clamp(p_levels(), 2, 32));
    let soft = p_softness();
    if (is_mask() || !is_color_channel()) {
        return vec4<f32>(vec3<f32>(pz_q(c.src.r, n, soft)), c.src.a);
    }
    var s = linear_to_srgb3(c.src.rgb);
    if (p_mode() == 1) {
        var hsv = rgb_to_hsv(s);
        hsv.z = pz_q(hsv.z, n, soft);
        s = hsv_to_rgb(hsv);
    } else {
        s = vec3<f32>(pz_q(s.r, n, soft), pz_q(s.g, n, soft), pz_q(s.b, n, soft));
    }
    return vec4<f32>(srgb_to_linear3(s), c.src.a);
}
