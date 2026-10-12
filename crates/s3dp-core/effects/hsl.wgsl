fn effect(c: Ctx) -> vec4<f32> {
    if (!is_color_channel()) {
        return vec4<f32>(vec3<f32>(clamp(c.src.r + p_value() * 0.5, 0.0, 1.0)), c.src.a);
    }
    // Work on display-referred values so the sliders feel like Photoshop's.
    var hsv = rgb_to_hsv(linear_to_srgb3(c.src.rgb));
    if (p_colorize()) {
        hsv.x = fract(p_hue() + 1.0);
        hsv.y = clamp(0.5 + p_saturation() * 0.5, 0.0, 1.0);
    } else {
        hsv.x = fract(hsv.x + p_hue() + 1.0);
        hsv.y = clamp(hsv.y * (1.0 + p_saturation()), 0.0, 1.0);
    }
    hsv.z = clamp(hsv.z + p_value() * 0.5, 0.0, 1.0);
    return vec4<f32>(srgb_to_linear3(hsv_to_rgb(hsv)), c.src.a);
}
