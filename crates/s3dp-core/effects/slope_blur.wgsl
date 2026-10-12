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

fn slope_dir(xy: vec2<i32>, p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    if (has_anchor0()) {
        let gx = luminance(anchor0(xy + vec2<i32>(1, 0)).rgb) - luminance(anchor0(xy - vec2<i32>(1, 0)).rgb);
        let gy = luminance(anchor0(xy + vec2<i32>(0, 1)).rgb) - luminance(anchor0(xy - vec2<i32>(0, 1)).rgb);
        let j = jacobian(xy);
        let g = j[0] * gx + j[1] * gy;
        let len = length(g);
        return select(vec3<f32>(0.0), g / len, len > 1e-9);
    }
    return wflow(p, n);
}

fn effect(c: Ctx) -> vec4<f32> {
    let steps = max(p_samples(), 1);
    let step_len = p_intensity() * radius() / f32(steps);
    var pos = vec2<f32>(c.xy);
    var wp = c.p;
    var wn = c.n;
    var sum = c.src;
    var lo = c.src;
    var hi = c.src;
    var count = 1.0;
    for (var i = 0; i < steps; i = i + 1) {
        let d = slope_dir(vec2<i32>(round(pos)), wp, wn);
        let step = surface_px_f(pos, wp, d * step_len);
        if (step.z < 0.5) { break; }
        let next = step.xy;
        let ixy = vec2<i32>(round(next));
        pos = next;
        wp = world(ixy);
        wn = world_normal(ixy);
        let v = sample_src_px(pos);
        sum = sum + v;
        lo = min(lo, v);
        hi = max(hi, v);
        count = count + 1.0;
    }
    switch p_mode() {
        case 1: { return lo; }
        case 2: { return hi; }
        default: { return mix(sum / count, hi, p_sharpness()); }
    }
}
