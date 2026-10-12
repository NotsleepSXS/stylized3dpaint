fn levels1(v: f32) -> f32 {
    var x = clamp((v - p_in_black()) / max(p_in_white() - p_in_black(), 1e-5), 0.0, 1.0);
    x = pow(x, 1.0 / max(p_gamma(), 1e-3));
    return mix(p_out_black(), p_out_white(), x);
}

fn effect(c: Ctx) -> vec4<f32> {
    var v = c.src;
    switch p_per() {
        case 1: { v.r = levels1(v.r); }
        case 2: { v.g = levels1(v.g); }
        case 3: { v.b = levels1(v.b); }
        default: { v = vec4<f32>(levels1(v.r), levels1(v.g), levels1(v.b), v.a); }
    }
    return v;
}
