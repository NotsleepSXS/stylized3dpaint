// Wood grain generator (spec §4, P1): growth rings around a world axis (Grain Direction through
// the object center, shifted by the seed), warped by 3D noise, plus fine pores stretched along
// the grain. Mask = dark latewood rings; on content: wood colors, roughness and height grooves.
// Everything is evaluated in world space (no UV seams).

// x: latewood ring (0..1), y: pores (0..1).
fn wg_pattern(p: vec3<f32>, hp: bool) -> vec2<f32> {
    let g = normalize(p_grain_dir() + vec3<f32>(0.0, 1e-6, 0.0));
    let sp = p_ring_spacing() * radius();
    let h = hash3(vec3<i32>(i32(seed()), 7, 13), 3u) - 0.5;
    let ctr = U.bbox_min.xyz + U.bbox_size.xyz * 0.5 + h * radius() * 0.6;
    let o = p - ctr;
    let along = dot(o, g);
    let r = o - g * along;
    // Low frequency warp stretched along the grain (knots / wavy rings).
    let q = r + g * along * 0.15;
    let oct = select(4, 1, hp);
    let warp = (fbm01(q, sp * 6.0, oct, seed() + 491u) - 0.5) * 2.0 * p_distortion() * sp * 4.0;
    let t = fract((length(r) + warp) / max(sp, 1e-6));
    // Early wood fades slowly, latewood ends sharply (asymmetric ring profile).
    let sharp = mix(1.5, 12.0, p_ring_sharpness());
    var ring = pow(t, sharp);
    if (hp) { ring = smoothstep(0.62, 0.7, t); }
    // Pores: thin streaks along the grain.
    var pores = 0.0;
    if (!hp && p_pores() > 0.0) {
        let pq = r + g * along * 0.04;
        let n = fbm01(pq, sp * 0.25, 2, seed() + 497u);
        pores = (1.0 - smoothstep(0.0, 0.06, abs(n - 0.5))) * p_pores();
    }
    return vec2<f32>(ring, pores);
}

fn effect(c: Ctx) -> vec4<f32> {
    let hp = p_hand_painted();
    let pt = wg_pattern(c.p, hp);
    let dark = clamp(max(pt.x, pt.y * 0.8) * p_intensity(), 0.0, 1.0);
    if (is_mask()) { return vec4<f32>(dark); }
    switch channel() {
        case 0u: { return vec4<f32>(mix(p_light_wood(), p_dark_wood(), dark), 1.0); }
        case 1u: { return vec4<f32>(vec3<f32>(clamp(c.src.r + (dark - 0.5) * p_roughness_variation(), 0.0, 1.0)), 1.0); }
        case 4u: {
            if (hp) { return vec4<f32>(c.src.rgb, 0.0); }
            return vec4<f32>(vec3<f32>(c.src.r - 0.08 * p_groove_depth() * dark), 1.0);
        }
        default: { return vec4<f32>(c.src.rgb, 1.0); }
    }
}
