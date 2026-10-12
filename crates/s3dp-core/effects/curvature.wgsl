// Curvature generator: baked curvature / edges / cavity, optionally averaged over a world-space
// disc (frequency) plus the curvature of the mesh surface itself.

fn cv_radius() -> f32 {
    switch p_frequency() {
        case 1: { return 0.004; }
        case 2: { return 0.01; }
        case 3: { return 0.02; }
        case 4: { return 0.04; }
        case 5: { return 0.08; }
        case 6: { return 0.15; }
        default: { return 0.0; }
    }
}

// x: convex, y: concave (both 0..1).
fn cv_sample(xy: vec2<i32>) -> vec2<f32> {
    let k = curvature(xy);
    let cx = max(edges(xy), clamp((k - 0.5) * 2.0, 0.0, 1.0));
    let cc = max(cavity(xy), clamp((0.5 - k) * 2.0, 0.0, 1.0));
    return vec2<f32>(cx, cc);
}

fn effect(c: Ctx) -> vec4<f32> {
    let rad = cv_radius() * radius();
    var acc = cv_sample(c.xy);
    var geo = 0.0;
    if (rad > 0.0) {
        let j = jacobian(c.xy);
        let fr = basis(c.n);
        let tw = texel_world(c.xy);
        var sum = vec2<f32>(0.0);
        var wsum = 0.0;
        var ksum = 0.0;
        var kw = 0.0;
        let rings = 4;
        for (var r = 0; r <= rings; r = r + 1) {
            let rr = f32(r) / f32(rings) * rad;
            let count = select(1, r * 6, r > 0);
            for (var k = 0; k < count; k = k + 1) {
                let a = (f32(k) + 0.5 * f32(r & 1)) / f32(count) * 2.0 * PI;
                let o = (fr[0] * cos(a) + fr[1] * sin(a)) * rr;
                let tap = surface_px(c.xy, c.p, o);
                if (tap.z < 0.5) { continue; }
                let ixy = vec2<i32>(round(tap.xy));
                let wp = world(ixy);
                let dp = wp - c.p;
                let w = exp(-2.0 * (rr / rad) * (rr / rad));
                sum = sum + cv_sample(ixy) * w;
                wsum = wsum + w;
                let d2 = dot(dp, dp);
                if (d2 > tw * tw) {
                    // Normal divergence: (n_s - n) . d / |d|^2 = 1 / R on a sphere of radius R.
                    ksum = ksum + dot(world_normal(ixy) - c.n, dp) / d2;
                    kw = kw + 1.0;
                }
            }
        }
        if (wsum > 0.0) { acc = sum / wsum; }
        if (kw > 0.0) { geo = ksum / kw * rad * 4.0; }
    }
    // Mesh-shape curvature, normalised to the averaging window.
    let cx = max(acc.x, clamp(geo, 0.0, 1.0) * p_shape());
    let cc = max(acc.y, clamp(-geo, 0.0, 1.0) * p_shape());
    var v = 0.0;
    switch p_mode() {
        case 1: { v = cc * p_intensity(); }
        case 2: { v = 0.5 + 0.5 * (cx - cc) * p_intensity(); }
        default: { v = cx * p_intensity(); }
    }
    v = clamp(v + p_luminosity() * 0.5, 0.0, 1.0);
    v = balance_contrast(v, 0.0, p_contrast());
    return vec4<f32>(v);
}
