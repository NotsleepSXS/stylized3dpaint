// Shared by warp effects: noise in 0..1 at world position p (seamless, resolution independent).
fn wnoise(p: vec3<f32>, n: vec3<f32>, offset: f32, octaves: i32) -> f32 {
    let scale = max(p_scale() * radius(), 1e-6);
    var q = p + vec3<f32>(offset * 31.7 * scale);
    if (p_noise_type() == 1) {
        let w = worley(q / scale, seed());
        return clamp(w.y - w.x, 0.0, 1.0);
    }
    if (p_noise_type() == 2) {
        var t = cross(n, vec3<f32>(0.0, 1.0, 0.0));
        if (dot(t, t) < 1e-6) { t = cross(n, vec3<f32>(1.0, 0.0, 0.0)); }
        t = normalize(t);
        q = q - t * dot(q, t) * (1.0 - 1.0 / max(p_stretch(), 1.0));
    }
    return fbm01(q, scale, octaves, seed());
}

// Flow direction in the tangent plane: the noise gradient rotated by 90 degrees.
// Two octaves are enough for the large-scale flow and keep slope blur affordable.
fn wflow(p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let oct = min(p_octaves(), 2);
    let h = max(p_scale() * radius(), 1e-6) * 0.15;
    let b = wnoise(p, n, 0.0, oct);
    let g = vec3<f32>(wnoise(p + vec3<f32>(h, 0.0, 0.0), n, 0.0, oct) - b, wnoise(p + vec3<f32>(0.0, h, 0.0), n, 0.0, oct) - b, wnoise(p + vec3<f32>(0.0, 0.0, h), n, 0.0, oct) - b);
    let tg = g - n * dot(g, n);
    let len = length(tg);
    if (len < 1e-8) { return vec3<f32>(0.0); }
    return cross(n, tg / len);
}

fn effect(c: Ctx) -> vec4<f32> {
    let steps = max(p_samples(), 1);
    let len_mod = 0.25 + 1.5 * wnoise(c.p, c.n, 3.0, p_octaves());
    let step_len = p_intensity() * radius() * len_mod / f32(steps);
    let along = normalize(p_direction() + vec3<f32>(0.0, 1e-6, 0.0));
    var pos = vec2<f32>(c.xy);
    var wn = c.n;
    var sum = c.src;
    var hi = c.src;
    var count = 1.0;
    for (var i = 0; i < steps; i = i + 1) {
        let d = along - wn * dot(along, wn);
        let next = pos + to_texels(jacobian(vec2<i32>(round(pos))), d * step_len);
        let ixy = vec2<i32>(round(next));
        if (!covered(ixy)) { break; }
        pos = next;
        wn = world_normal(ixy);
        let v = sample_src_px(pos);
        sum = sum + v;
        hi = max(hi, v);
        count = count + 1.0;
    }
    return mix(sum / count, hi, p_sharpness());
}
