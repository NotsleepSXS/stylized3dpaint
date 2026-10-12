// Thickness generator: thin parts white (baked map stores 0 = thin, 1 = thick).
// Balance is a gain (x1/8 .. x8), contrast an S-curve that keeps 0 and 1 fixed.

fn effect(c: Ctx) -> vec4<f32> {
    var v = clamp((1.0 - thickness(c.xy)) * exp2(p_balance() * 3.0), 0.0, 1.0);
    let k = 1.0 + p_contrast() * 4.0;
    v = pow(v, k) / max(pow(v, k) + pow(1.0 - v, k), 1e-6);
    return vec4<f32>(v);
}
