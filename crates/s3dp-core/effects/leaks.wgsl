// Leaks generator: walks UP along the surface from each texel (re-projecting world +Y onto the
// local tangent plane at every step) and picks up the source with a fade over the length.
// Streak columns are 3D noise in world X/Z, stretched vertically.

const LK_STEPS: i32 = 24;

fn lk_source(xy: vec2<i32>) -> f32 {
    switch p_source() {
        case 1: { return smoothstep(0.5, 0.9, world_normal(xy).y); }
        case 2: {
            if (!has_anchor0()) { return 0.0; }
            return clamp(anchor0(xy).r, 0.0, 1.0);
        }
        default: {
            let k = max(edges(xy), clamp((curvature(xy) - 0.5) * 2.0, 0.0, 1.0));
            return smoothstep(0.15, 0.6, k);
        }
    }
}

// World up projected on the tangent plane of n (zero on horizontal surfaces).
fn lk_up(n: vec3<f32>) -> vec3<f32> { return vec3<f32>(0.0, 1.0, 0.0) - n * n.y; }

fn effect(c: Ctx) -> vec4<f32> {
    let up0 = lk_up(c.n);
    let vertical = smoothstep(0.15, 0.5, length(up0));
    if (vertical <= 0.0) { return vec4<f32>(0.0); }
    // Streak columns and per-column length.
    let cw = p_width() * radius();
    let q = vec3<f32>(c.p.x, c.p.y * 0.04, c.p.z);
    let col = fbm01(q, cw, 2, seed() + 41u);
    let a = p_amount();
    let streak = smoothstep(1.0 - a - 0.06, 1.0 - a + 0.06, col);
    if (streak <= 0.0) { return vec4<f32>(0.0); }
    let lr = fbm01(q, cw * 3.0, 1, seed() + 43u);
    let len = p_length() * radius() * mix(1.0, 0.15 + 1.7 * lr, p_randomness());
    let dt = len / f32(LK_STEPS);
    var fxy = vec2<f32>(c.xy);
    var xy = c.xy;
    var pos = c.p;
    var best = 0.0;
    for (var i = 1; i <= LK_STEPS; i = i + 1) {
        let n = world_normal(xy);
        let u = lk_up(n);
        let ul = length(u);
        if (ul < 0.05) { break; }
        let step = surface_px_f(fxy, pos, u / ul * dt);
        if (step.z < 0.5) { break; }
        fxy = step.xy;
        let nxy = vec2<i32>(round(fxy));
        let wp = world(nxy);
        // Stop at UV seams / jumps to another part of the mesh.
        if (distance(wp, pos) > dt * 3.0 + texel_world(xy) * 2.0) { break; }
        pos = wp;
        xy = nxy;
        let t = f32(i) / f32(LK_STEPS);
        best = max(best, lk_source(xy) * (1.0 - t * t));
    }
    return vec4<f32>(clamp(best * streak * vertical, 0.0, 1.0));
}
