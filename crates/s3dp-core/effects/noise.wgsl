// Noise generator: 3D world-space noise (seamless, resolution independent).

fn effect(c: Ctx) -> vec4<f32> {
    let sc = max(p_scale() * radius(), 1e-5);
    let s = seed();
    var p = c.p;
    if (p_distortion() > 0.0) {
        let w = vec3<f32>(fbm01(p, sc, 2, s + 61u), fbm01(p, sc, 2, s + 62u), fbm01(p, sc, 2, s + 63u)) - 0.5;
        p = p + w * sc * 2.0 * p_distortion();
    }
    let q = p / sc;
    var v = 0.0;
    switch p_kind() {
        case 1: { v = clamp(worley(q, s).x, 0.0, 1.0); }
        case 2: {
            let w = worley(q, s);
            v = 1.0 - smoothstep(0.0, 0.12, w.y - w.x);
        }
        case 3: { v = grunge(p, 4, sc, s); }
        case 4: {
            // Billowy clouds: domain-warped fractal noise.
            let wp = p + (vec3<f32>(fbm01(p, sc * 2.0, 2, s + 71u), fbm01(p, sc * 2.0, 2, s + 72u), 0.5) - 0.5) * sc;
            v = grunge(wp, 1, sc, s);
            v = smoothstep(0.2, 0.8, v);
        }
        default: { v = fbm01(p, sc, p_octaves(), s); }
    }
    return vec4<f32>(v);
}
