// Cavity darken (spec §6.4): concave mask (cavity + concave curvature + AO) averaged over a
// small world disc, multiplies a tint color and rotates the hue towards cool / warm.

fn cd_sample(xy: vec2<i32>) -> f32 {
    let kv = curvature(xy);
    let cav = max(cavity(xy), clamp((0.5 - kv) * 2.0, 0.0, 1.0));
    return max(cav, (1.0 - ao(xy)) * p_ao_amount());
}

fn cd_shift_hue(h: f32, goal: f32, t: f32) -> f32 {
    var d = goal - h;
    d = d - round(d);
    return fract(h + d * t + 1.0);
}

fn effect(c: Ctx) -> vec4<f32> {
    if (is_mask() || channel() != 0u) { return c.src; }
    let rad = p_spread() * radius();
    var v = cd_sample(c.xy);
    if (rad > 0.0) {
        let j = jacobian(c.xy);
        let fr = basis(c.n);
        let tw = texel_world(c.xy);
        for (var r = 1; r <= 2; r = r + 1) {
            let rr = f32(r) * 0.5 * rad;
            for (var k = 0; k < 6 * r; k = k + 1) {
                let a = (f32(k) + 0.5 * f32(r & 1)) / f32(6 * r) * 2.0 * PI;
                let ixy = vec2<i32>(round(vec2<f32>(c.xy) + to_texels(j, (fr[0] * cos(a) + fr[1] * sin(a)) * rr)));
                if (!covered(ixy) || distance(world(ixy), c.p) > rr * 2.0 + tw * 2.0) { continue; }
                v = max(v, cd_sample(ixy) * (1.0 - 0.4 * f32(r) * 0.5));
            }
        }
    }
    let kc = p_contrast();
    let m = clamp(smoothstep(0.2 * kc, 1.0 - 0.6 * kc, v) * p_intensity(), 0.0, 1.0);
    if (m <= 0.0) { return c.src; }
    var hsv = rgb_to_hsv(linear_to_srgb3(c.src.rgb));
    let hs = p_hue_shift();
    // Cool goal: blue 0.62, warm goal: red-orange 0.03.
    hsv.x = cd_shift_hue(hsv.x, select(0.03, 0.62, hs >= 0.0), abs(hs) * 0.5 * m);
    hsv.y = clamp(hsv.y * (1.0 + p_saturation() * m), 0.0, 1.0);
    var col = srgb_to_linear3(hsv_to_rgb(hsv));
    col = mix(col, col * p_color(), m);
    return vec4<f32>(col, c.src.a);
}
