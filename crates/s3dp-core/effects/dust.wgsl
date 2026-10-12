// Dust generator: up-facing surfaces, boosted in occluded corners, thresholded 3D noise breakup.

fn effect(c: Ctx) -> vec4<f32> {
    let ang = degrees(acos(clamp(c.n.y, -1.0, 1.0)));
    let facing = 1.0 - smoothstep(p_angle() * 0.5, p_angle(), ang);
    let occ = 1.0 - ao(c.xy);
    let base = facing * mix(1.0, 0.6 + occ * 1.2, p_ao_mask());
    let n = fbm01(c.p, p_noise_scale() * radius(), 5, seed() + 23u);
    let score = base * mix(0.35, 1.0, n);
    let t = 1.0 - p_amount();
    let v = smoothstep(t * 0.9, t * 0.9 + 0.3, score);
    return vec4<f32>(v);
}
