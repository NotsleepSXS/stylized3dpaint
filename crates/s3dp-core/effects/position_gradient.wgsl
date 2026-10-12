fn effect(c: Ctx) -> vec4<f32> {
    var d = vec3<f32>(0.0, 1.0, 0.0);
    switch p_axis() {
        case 1: { d = vec3<f32>(1.0, 0.0, 0.0); }
        case 2: { d = vec3<f32>(0.0, 0.0, 1.0); }
        case 3: { d = normalize(p_direction() + vec3<f32>(0.0, 1e-6, 0.0)); }
        default: {}
    }
    // Project the bounding box on the direction to normalize the gradient to 0..1.
    let lo = U.bbox_min.xyz;
    let hi = U.bbox_min.xyz + U.bbox_size.xyz;
    let a = dot(select(hi, lo, d > vec3<f32>(0.0)), d);
    let b = dot(select(lo, hi, d > vec3<f32>(0.0)), d);
    var t = (dot(c.p, d) - a) / max(b - a, 1e-6);
    t = balance_contrast(t, p_offset(), p_contrast());
    if (p_invert()) { t = 1.0 - t; }
    return vec4<f32>(t);
}
