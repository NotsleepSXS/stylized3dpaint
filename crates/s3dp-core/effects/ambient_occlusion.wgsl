// Ambient occlusion generator: 1 - AO, widened by a seam-aware world-space min/average.

fn effect(c: Ctx) -> vec4<f32> {
    var occ = 1.0 - ao(c.xy);
    let rad = p_spread() * radius();
    if (rad > 0.0) {
        let j = jacobian(c.xy);
        let fr = basis(c.n);
        let tw = texel_world(c.xy);
        var sum = 0.0;
        var wsum = 0.0;
        var peak = occ;
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
                let w = exp(-2.0 * (rr / rad) * (rr / rad));
                let s = 1.0 - ao(ixy);
                sum = sum + s * w;
                wsum = wsum + w;
                peak = max(peak, s * w);
            }
        }
        // Widening: shadows grow outwards (weighted max) but stay smooth (average).
        if (wsum > 0.0) { occ = max(occ, 0.5 * (sum / wsum + peak)); }
    }
    // Balance is a gain (x1/8 .. x8): baked occlusion is often faint; contrast is an S-curve
    // that keeps open areas at exactly 0.
    var v = clamp(occ * exp2(p_balance() * 3.0), 0.0, 1.0);
    let k = 1.0 + p_contrast() * 4.0;
    v = pow(v, k) / max(pow(v, k) + pow(1.0 - v, k), 1e-6);
    if (p_invert()) { v = 1.0 - v; }
    return vec4<f32>(v);
}
