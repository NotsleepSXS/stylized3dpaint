// Palette lock (spec §6.5): nearest palette color by a luma-weighted sRGB distance; optional
// dithering between the two nearest colors from a world-space hash (no UV seams).

fn pl_color(i: i32) -> vec3<f32> {
    switch i {
        case 0: { return p_c1(); }
        case 1: { return p_c2(); }
        case 2: { return p_c3(); }
        case 3: { return p_c4(); }
        case 4: { return p_c5(); }
        case 5: { return p_c6(); }
        case 6: { return p_c7(); }
        default: { return p_c8(); }
    }
}

fn pl_dist(a: vec3<f32>, b: vec3<f32>) -> f32 {
    let d = a - b;
    return dot(d * d, vec3<f32>(0.3, 0.59, 0.11));
}

fn effect(c: Ctx) -> vec4<f32> {
    if (is_mask() || channel() != 0u) { return c.src; }
    let s = linear_to_srgb3(c.src.rgb);
    var b1 = 1e9;
    var b2 = 1e9;
    var i1 = 0;
    var i2 = 0;
    for (var i = 0; i < clamp(p_count(), 2, 8); i = i + 1) {
        let d = pl_dist(s, linear_to_srgb3(pl_color(i)));
        if (d < b1) { b2 = b1; i2 = i1; b1 = d; i1 = i; } else if (d < b2) { b2 = d; i2 = i; }
    }
    var pick = i1;
    if (p_dither() && b2 < 1e8) {
        let r = sqrt(b1) / max(sqrt(b1) + sqrt(b2), 1e-6);
        let cell = vec3<i32>(floor(c.p / max(p_dither_scale() * radius(), 1e-6)));
        if (hash1(cell, seed() + 77u) < r) { pick = i2; }
    }
    return vec4<f32>(pl_color(pick), c.src.a);
}
