// Fake light generator: wrapped Lambert + Blinn-ish highlight around the light direction.

fn effect(c: Ctx) -> vec4<f32> {
    let h = radians(p_h_angle());
    let e = radians(p_v_angle());
    let l = normalize(vec3<f32>(cos(e) * cos(h), sin(e), cos(e) * sin(h)));
    let d = dot(c.n, l);
    // Half-Lambert sharpened by the attenuation.
    let wrap = clamp(d * 0.5 + 0.5, 0.0, 1.0);
    let hard = smoothstep(-0.05, 0.25, d) * clamp(d * 0.7 + 0.3, 0.0, 1.0);
    var v = mix(wrap * wrap, hard, p_attenuation());
    let expo = mix(4.0, 256.0, p_glossiness() * p_glossiness());
    v = v + p_highlight() * pow(max(d, 0.0), expo);
    v = v * mix(1.0, ao(c.xy), p_occlusion());
    return vec4<f32>(clamp(v, 0.0, 1.0));
}
