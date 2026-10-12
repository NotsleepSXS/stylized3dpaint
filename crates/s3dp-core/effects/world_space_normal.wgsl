// World space normal generator: angle between the surface normal and a direction.

fn effect(c: Ctx) -> vec4<f32> {
    let d = normalize(p_direction() + vec3<f32>(0.0, 1e-6, 0.0));
    let ang = degrees(acos(clamp(dot(c.n, d), -1.0, 1.0)));
    let f = max(p_falloff(), 0.01);
    let v = 1.0 - smoothstep(p_angle() - f * 0.5, p_angle() + f * 0.5, ang);
    return vec4<f32>(v);
}
