// Stylized baked lighting (spec §6.2): albedo * (sun mixed towards a tinted shadow color, plus
// sky / ground / ambient bounce, rim) * AO. Writes Base Color only.

fn effect(c: Ctx) -> vec4<f32> {
    if (is_mask() || channel() != 0u) { return c.src; }
    let l = normalize(p_sun_dir() + vec3<f32>(0.0, 1e-6, 0.0));
    let ndl = dot(c.n, l);
    // Half-Lambert wrap, sharpened towards a crisp terminator by the hardness.
    let wrap = ndl * 0.5 + 0.5;
    let hd = clamp(p_hardness(), 0.0, 1.0);
    let soft = mix(0.5, 0.015, hd);
    var lit = smoothstep(0.5 - soft, 0.5 + soft, mix(wrap * wrap, wrap, hd));
    // Baked sky light as a soft cast-shadow proxy.
    lit = lit * mix(1.0, smoothstep(0.15, 0.85, sky(c.xy)), p_sky_shadow());
    var light = mix(p_shadow_color(), p_sun_color() * p_intensity(), lit);
    let hemi = mix(p_ground_color(), p_sky_color(), c.n.y * 0.5 + 0.5);
    light = light + hemi * p_hemi() + p_ambient_color() * 0.5;
    // Rim: back light from the side opposite the sun, plus convex edges catching light.
    let rd = normalize(vec3<f32>(-l.x, 0.35, -l.z) + vec3<f32>(0.0, 1e-4, 0.0));
    let w = clamp(p_rim_width(), 0.0, 1.0);
    let back = smoothstep(1.0 - w * 0.9 - 0.05, 1.0, dot(c.n, rd)) * (1.0 - lit * 0.5);
    let kv = curvature(c.xy);
    let edge = max(edges(c.xy), clamp((kv - 0.5) * 2.0, 0.0, 1.0)) * smoothstep(-0.2, 0.6, c.n.y) * w;
    light = light + p_rim_color() * max(back, edge) * p_rim_strength();
    light = light * mix(1.0, ao(c.xy), p_ao_influence());
    return vec4<f32>(max(c.src.rgb * light, vec3<f32>(0.0)), c.src.a);
}
