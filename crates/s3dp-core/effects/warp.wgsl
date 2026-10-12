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
    var o: vec3<f32>;
    if (has_anchor0()) {
        // Gradient of the anchor (in texels) used as the displacement direction.
        let gx = luminance(anchor0(c.xy + vec2<i32>(1, 0)).rgb) - luminance(anchor0(c.xy - vec2<i32>(1, 0)).rgb);
        let gy = luminance(anchor0(c.xy + vec2<i32>(0, 1)).rgb) - luminance(anchor0(c.xy - vec2<i32>(0, 1)).rgb);
        let j = jacobian(c.xy);
        o = normalize(j[0] * gx + j[1] * gy + vec3<f32>(1e-9));
    } else {
        o = vec3<f32>(wnoise(c.p, c.n, 0.0, p_octaves()), wnoise(c.p, c.n, 1.0, p_octaves()), wnoise(c.p, c.n, 2.0, p_octaves())) * 2.0 - 1.0;
    }
    o = (o - c.n * dot(o, c.n)) * p_intensity() * radius();
    // Seam-aware (continues across UV seams in seamless mode).
    let dest = surface_px(c.xy, c.p, o);
    if (dest.z < 0.5) { return c.src; }
    return sample_src_px(dest.xy);
}
