// UV border generator: ray search for the nearest uncovered texel in 16 directions.

fn effect(c: Ctx) -> vec4<f32> {
    let r = max(p_distance() * f32(max(c.size.x, c.size.y)), 1.0);
    let steps = clamp(i32(ceil(r)), 1, 24);
    var dmin = r;
    for (var k = 0; k < 16; k = k + 1) {
        let a = f32(k) / 16.0 * 2.0 * PI;
        let dir = vec2<f32>(cos(a), sin(a));
        for (var s = 1; s <= steps; s = s + 1) {
            let d = f32(s) / f32(steps) * r;
            if (d >= dmin) { break; }
            let ixy = vec2<i32>(round(vec2<f32>(c.xy) + dir * d));
            if (!covered(ixy)) { dmin = d; break; }
        }
    }
    // Border sits half a texel outside the last covered texel.
    let v = 1.0 - clamp((dmin - 0.5) / max(r - 0.5, 1e-3), 0.0, 1.0);
    return vec4<f32>(balance_contrast(v, 0.0, p_contrast()));
}
