// Toon shading (spec §6.3): quantized wrapped diffuse -> shadow / mid / light ramp, a hard
// specular spot towards the light and ink outlines from the baked curvature.

fn ts_ramp(t: f32) -> vec3<f32> {
    if (t < 0.5) { return mix(p_shadow_color(), p_mid_color(), t * 2.0); }
    return mix(p_mid_color(), p_light_color(), t * 2.0 - 1.0);
}

fn effect(c: Ctx) -> vec4<f32> {
    if (is_mask() || channel() != 0u) { return c.src; }
    let l = normalize(p_light_dir() + vec3<f32>(0.0, 1e-6, 0.0));
    let ndl = dot(c.n, l);
    let d = clamp((ndl * 0.5 + 0.5) * mix(1.0, ao(c.xy), p_ao_influence()), 0.0, 1.0);
    let n = f32(clamp(p_steps(), 2, 5));
    let x = d * n;
    let soft = max(p_softness(), 0.01);
    // Sum of soft steps centered on each band boundary.
    var band = 0.0;
    for (var k = 1; k < i32(n); k = k + 1) {
        band = band + smoothstep(f32(k) - soft * 0.5, f32(k) + soft * 0.5, x);
    }
    let t = band / (n - 1.0);
    let tone = ts_ramp(t);
    var col = select(c.src.rgb * tone, tone, p_mode() == 1);
    if (p_spec_size() > 0.0) {
        let th = 1.0 - p_spec_size() * 0.25;
        let sw = p_spec_threshold() * 0.1 + 0.002;
        let sp = smoothstep(th - sw, th + sw, ndl);
        col = mix(col, p_spec_color(), sp);
    }
    let w = p_outline_width();
    if (w > 0.0) {
        let kv = curvature(c.xy);
        let k = max(abs(kv - 0.5) * 2.0, max(edges(c.xy), cavity(c.xy)));
        let th = 1.0 - w * 0.8;
        col = mix(col, p_outline_color(), smoothstep(th - 0.04, th + 0.04, k));
    }
    return vec4<f32>(col, c.src.a);
}
