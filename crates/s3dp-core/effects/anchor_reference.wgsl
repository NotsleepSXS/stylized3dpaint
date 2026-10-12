fn effect(c: Ctx) -> vec4<f32> {
    if (!has_anchor0()) { return c.src; }
    let a = anchor0(c.xy);
    var v = a.r;
    switch p_read() {
        case 1: { v = luminance(a.rgb); }
        case 2: { v = a.a; }
        default: {}
    }
    if (p_invert()) { v = 1.0 - v; }
    if (is_mask() || !is_color_channel()) { return vec4<f32>(v, v, v, c.src.a); }
    return vec4<f32>(select(vec3<f32>(v), a.rgb, p_read() == 0), c.src.a);
}
