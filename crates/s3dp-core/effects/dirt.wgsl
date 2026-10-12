// Dirt generator: occlusion + concave curvature + bottom gradient, broken up by 3D noise.

fn effect(c: Ctx) -> vec4<f32> {
    let occ = 1.0 - ao(c.xy);
    let k = curvature(c.xy);
    let concave = max(cavity(c.xy), clamp((0.5 - k) * 2.0, 0.0, 1.0));
    let bottom = 1.0 - position01(c.p).y;
    let low = bottom * bottom;
    // Screen-combine the sources.
    var d = 1.0 - (1.0 - occ * p_ao_mask()) * (1.0 - concave * p_curvature_mask()) * (1.0 - low * p_top_down());
    let n = fbm01(c.p, p_breakup_scale() * radius(), 4, seed() + 17u);
    d = d * mix(1.0, n * 2.0, p_breakup());
    // Dirt level shifts the threshold: 0 = none, 1 = everywhere.
    let v = balance_contrast(clamp(d, 0.0, 1.0), p_level() * 2.0 - 1.0, p_contrast());
    return vec4<f32>(v);
}
