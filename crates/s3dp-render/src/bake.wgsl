// Mesh map baker: ray tracing against a BVH in compute shaders.
//
// Entry points (each pipeline uses its own subset of bindings):
//   project  - finds the detail surface for every texel (high-poly or the mesh itself)
//              and writes the tangent-space normal map
//   maps     - AO, edges, cavity, curvature, thickness and sky light for one tile
//   soften   - geometry-aware texture-space blur (stylized softening), one axis per pass

struct BakeParams {
    // xy: tile origin, zw: tile size (texels)
    tile: vec4<u32>,
    // ao, edge, thickness, sky sample counts
    counts: vec4<u32>,
    // ao distance, edge width, thickness distance, ray epsilon (world units)
    dist: vec4<f32>,
    // xyz: direction towards the sky light, w: cos(cone half angle)
    sky: vec4<f32>,
    // x: cage front, y: cage back, z: stylized (0/1), w: has high-poly (0/1)
    cage: vec4<f32>,
    // x: smoothing radius (world), y: scene radius, z: smooth axis (0 = x, 1 = y),
    // w: smoothed target (0 = maps a, 1 = maps b)
    extra: vec4<f32>,
};

struct Node {
    min: vec3<f32>,
    first: u32,
    max: vec3<f32>,
    count: u32,
};

struct Tri {
    v0: vec4<f32>,
    v1: vec4<f32>,
    v2: vec4<f32>,
    n0: vec4<f32>,
    n1: vec4<f32>,
    n2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> P: BakeParams;
@group(0) @binding(1) var<storage, read> nodes: array<Node>;
@group(0) @binding(2) var<storage, read> tris: array<Tri>;

// Low-poly G-buffer (UV space).
@group(1) @binding(0) var g_pos: texture_2d<f32>;      // xyz world, w coverage
@group(1) @binding(1) var g_nrm: texture_2d<f32>;      // interpolated normal
@group(1) @binding(2) var g_tan: texture_2d<f32>;      // tangent xyz, w bitangent sign
@group(1) @binding(3) var g_face: texture_2d<f32>;     // geometric normal

// project outputs
@group(2) @binding(0) var o_surf_pos: texture_storage_2d<rgba32float, write>;
@group(2) @binding(1) var o_surf_nrm: texture_storage_2d<rgba16float, write>;
@group(2) @binding(2) var o_normal_map: texture_storage_2d<rgba16float, write>;

// maps inputs / outputs
@group(2) @binding(3) var i_surf_pos: texture_2d<f32>;
@group(2) @binding(4) var i_surf_nrm: texture_2d<f32>;
@group(2) @binding(5) var o_maps_a: texture_storage_2d<rgba16float, write>;  // ao, edges, cavity, thickness
@group(2) @binding(6) var o_maps_b: texture_storage_2d<rgba16float, write>;  // sky, curvature

// soften input / output
@group(2) @binding(7) var s_src: texture_2d<f32>;
@group(2) @binding(8) var s_dst: texture_storage_2d<rgba16float, write>;

const PI: f32 = 3.14159265;
const MISS: f32 = 1e30;
const STACK: u32 = 64u;

// ---------------------------------------------------------------- random numbers

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn rand2(p: vec2<u32>) -> vec2<f32> {
    let a = pcg(p.x ^ pcg(p.y));
    let b = pcg(a);
    return vec2<f32>(f32(a & 0xffffffu), f32(b & 0xffffffu)) / 16777216.0;
}

// R2 low-discrepancy sequence with a per-texel Cranley-Patterson rotation.
fn r2(i: u32, rot: vec2<f32>) -> vec2<f32> {
    return fract(rot + f32(i) * vec2<f32>(0.7548776662, 0.5698402910));
}

// Orthonormal basis with `n` as the third column (Duff et al.).
fn basis(n: vec3<f32>) -> mat3x3<f32> {
    let s = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    let t = vec3<f32>(1.0 + s * n.x * n.x * a, s * b, -s * n.x);
    let bt = vec3<f32>(b, s + n.y * n.y * a, -n.y);
    return mat3x3<f32>(t, bt, n);
}

fn cosine_dir(n: vec3<f32>, u: vec2<f32>) -> vec3<f32> {
    let r = sqrt(u.x);
    let phi = 2.0 * PI * u.y;
    let l = vec3<f32>(r * cos(phi), r * sin(phi), sqrt(max(1.0 - u.x, 0.0)));
    return normalize(basis(n) * l);
}

fn cone_dir(axis: vec3<f32>, cos_max: f32, u: vec2<f32>) -> vec3<f32> {
    let c = 1.0 - u.x * (1.0 - cos_max);
    let s = sqrt(max(1.0 - c * c, 0.0));
    let phi = 2.0 * PI * u.y;
    return normalize(basis(axis) * vec3<f32>(s * cos(phi), s * sin(phi), c));
}

// ---------------------------------------------------------------- ray tracing

struct Hit {
    t: f32,
    tri: u32,
    u: f32,
    v: f32,
};

fn hit_box(o: vec3<f32>, inv: vec3<f32>, n: Node, tmax: f32) -> bool {
    let t0 = (n.min - o) * inv;
    let t1 = (n.max - o) * inv;
    let lo = max(max(min(t0.x, t1.x), min(t0.y, t1.y)), min(t0.z, t1.z));
    let hi = min(min(max(t0.x, t1.x), max(t0.y, t1.y)), max(t0.z, t1.z));
    return hi >= max(lo, 0.0) && lo <= tmax;
}

// Closest hit within (tmin, tmax); with `any` the first hit found is returned.
fn trace(o: vec3<f32>, d: vec3<f32>, tmin: f32, tmax: f32, any_hit: bool) -> Hit {
    let safe = select(d, vec3<f32>(1e-9), abs(d) < vec3<f32>(1e-9));
    let inv = 1.0 / safe;
    var best = Hit(MISS, 0u, 0.0, 0.0);
    var limit = tmax;
    var stack: array<u32, STACK>;
    var sp = 1u;
    stack[0] = 0u;
    loop {
        if (sp == 0u) { break; }
        sp = sp - 1u;
        let index = stack[sp];
        let node = nodes[index];
        if (!hit_box(o, inv, node, limit)) { continue; }
        if (node.count > 0u) {
            for (var i = node.first; i < node.first + node.count; i = i + 1u) {
                let tri = tris[i];
                let e1 = tri.v1.xyz - tri.v0.xyz;
                let e2 = tri.v2.xyz - tri.v0.xyz;
                let pv = cross(d, e2);
                let det = dot(e1, pv);
                if (abs(det) < 1e-14) { continue; }
                let inv_det = 1.0 / det;
                let s = o - tri.v0.xyz;
                let u = dot(s, pv) * inv_det;
                if (u < 0.0 || u > 1.0) { continue; }
                let q = cross(s, e1);
                let v = dot(d, q) * inv_det;
                if (v < 0.0 || u + v > 1.0) { continue; }
                let t = dot(e2, q) * inv_det;
                if (t > tmin && t < limit) {
                    limit = t;
                    best = Hit(t, i, u, v);
                    if (any_hit) { return best; }
                }
            }
        } else if (sp + 2u <= STACK) {
            // Left child follows its parent; `first` is the right child.
            stack[sp] = node.first;
            stack[sp + 1u] = index + 1u;
            sp = sp + 2u;
        }
    }
    return best;
}

fn hit_normal(h: Hit) -> vec3<f32> {
    let tri = tris[h.tri];
    return normalize(tri.n0.xyz * (1.0 - h.u - h.v) + tri.n1.xyz * h.u + tri.n2.xyz * h.v);
}

// ---------------------------------------------------------------- project

@compute @workgroup_size(8, 8)
fn project(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(g_pos);
    if (id.x >= size.x || id.y >= size.y) { return; }
    let xy = vec2<i32>(id.xy);
    let gp = textureLoad(g_pos, xy, 0);
    if (gp.w < 0.5) {
        textureStore(o_surf_pos, xy, vec4<f32>(0.0));
        textureStore(o_surf_nrm, xy, vec4<f32>(0.0, 0.0, 1.0, 0.0));
        textureStore(o_normal_map, xy, vec4<f32>(0.5, 0.5, 1.0, 1.0));
        return;
    }
    let n = normalize(textureLoad(g_nrm, xy, 0).xyz);
    var p = gp.xyz;
    var hn = n;
    if (P.cage.w > 0.5) {
        // Search the high-poly along the normal: start in front, look through to behind.
        let o = p + n * P.cage.x;
        let h = trace(o, -n, 0.0, P.cage.x + P.cage.y, false);
        if (h.t < MISS) {
            p = o - n * h.t;
            hn = hit_normal(h);
            if (dot(hn, n) < 0.0) { hn = -hn; }
        }
    }
    textureStore(o_surf_pos, xy, vec4<f32>(p, 1.0));
    textureStore(o_surf_nrm, xy, vec4<f32>(hn, 1.0));

    // Tangent-space normal (MikkTSpace frame, OpenGL +Y).
    let tg = textureLoad(g_tan, xy, 0);
    var t = tg.xyz - n * dot(tg.xyz, n);
    t = select(basis(n)[0], normalize(t), dot(t, t) > 1e-12);
    let bsign = select(1.0, -1.0, tg.w < 0.0);
    let b = bsign * cross(n, t);
    let ts = normalize(vec3<f32>(dot(hn, t), dot(hn, b), dot(hn, n)));
    textureStore(o_normal_map, xy, vec4<f32>(ts * 0.5 + 0.5, 1.0));
}

// ---------------------------------------------------------------- maps

@compute @workgroup_size(8, 8)
fn maps(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= P.tile.z || gid.y >= P.tile.w) { return; }
    let texel = gid.xy + P.tile.xy;
    let size = textureDimensions(i_surf_pos);
    if (texel.x >= size.x || texel.y >= size.y) { return; }
    let xy = vec2<i32>(texel);
    let sp = textureLoad(i_surf_pos, xy, 0);
    if (sp.w < 0.5) {
        textureStore(o_maps_a, xy, vec4<f32>(1.0, 0.0, 0.0, 0.5));
        textureStore(o_maps_b, xy, vec4<f32>(1.0, 0.5, 0.0, 0.0));
        return;
    }
    let p = sp.xyz;
    let n = normalize(textureLoad(i_surf_nrm, xy, 0).xyz);
    // Offset along the geometric normal of the surface we start on.
    var geo = n;
    if (P.cage.w < 0.5) {
        geo = normalize(textureLoad(g_face, xy, 0).xyz);
        if (dot(geo, n) < 0.0) { geo = -geo; }
    }
    let eps = P.dist.w;
    let origin = p + geo * eps;
    let rot = rand2(texel);
    let stylized = P.cage.z > 0.5;

    // Ambient occlusion.
    var occl = 0.0;
    let ao_n = P.counts.x;
    for (var i = 0u; i < ao_n; i = i + 1u) {
        var d = cosine_dir(n, r2(i, rot));
        if (dot(d, geo) < 0.0) { d = d - 2.0 * dot(d, geo) * geo; }
        let h = trace(origin, d, eps, P.dist.x, !stylized);
        if (h.t < MISS) {
            if (stylized) {
                let f = 1.0 - h.t / P.dist.x;
                occl = occl + f * f;
            } else {
                occl = occl + 1.0;
            }
        }
    }
    let ao = 1.0 - occl / f32(max(ao_n, 1u));

    // Edges / cavity / curvature: how much the surface normal turns over a small
    // neighbourhood. For every probe direction `t` (tangent plane of the shading normal)
    // a nearby surface point q is found and its normal compared with ours:
    //   s = dot(n_q - n, normalize(q - p))  (~ turning angle; > 0 convex, < 0 concave).
    // This is symmetric for convex and concave shapes and uses interpolated normals at
    // both ends, so faceted smooth-shaded meshes read as smooth. Radii are spread over
    // 0.25..1 x width for a soft falloff around hard edges.
    // (The stylized look is wider via the preset's larger width.)
    let w = P.dist.y;
    let frame = basis(n);
    var convex = 0.0;
    var concave = 0.0;
    let k = max(P.counts.y, 1u);
    for (var i = 0u; i < k; i = i + 1u) {
        // Fixed (not per-texel random) directions and radii: the result is then a
        // continuous function of the surface, noise-free and identical on both sides of
        // a UV seam, so it needs no texture-space blur.
        let a = (f32(i) + 0.5) / f32(k) * 2.0 * PI;
        let r = w * mix(0.25, 1.0, fract(0.5 + f32(i) * 0.6180339887));
        let t = frame * vec3<f32>(cos(a), sin(a), 0.0);
        // Look down onto the surface one radius away.
        var s = 1.0;
        let probe = p + t * r + n * r;
        let ground = trace(probe, -n, 0.0, 2.0 * r, false);
        if (ground.t < MISS) {
            let gn = hit_normal(ground);
            if (dot(gn, n) <= 0.0) {
                // Started inside the mesh: the surface folds up within reach.
                s = -1.0;
            } else {
                let q = probe - n * ground.t;
                s = dot(gn - n, normalize(q - p));
            }
        }
        // Otherwise (miss) the surface drops away by more than the radius: convex edge.
        //
        // A wall in the way along the triangle plane: concave corner. Rays run along the
        // geometric plane (not the shading tangent) so they never graze the starting
        // triangle; nearly parallel hits (gentle creases between facets) fade out
        // smoothly in favour of the height probe, as do walls near the probe radius.
        var tw = t;
        if (true) {
            let wall = trace(origin, tw, eps, r, false);
            if (wall.t < MISS) {
                let wn = hit_normal(wall);
                let facing = smoothstep(0.2, 0.5, -dot(wn, tw));
                let near = 1.0 - smoothstep(0.7, 1.0, wall.t / r);
                s = mix(s, dot(wn - n, tw), facing * near);
            }
        }
        s = clamp(s, -1.0, 1.0);
        convex = convex + max(s, 0.0);
        concave = concave + max(-s, 0.0);
    }
    convex = convex / f32(k);
    concave = concave / f32(k);
    // Soft, threshold-free gain: small responses stay small, strong edges saturate.
    let gain = select(4.0, 3.0, stylized);
    // The masks get a smooth knee (x^2 / (x + knee)) so gently curved areas stay dark
    // while edges and creases keep their full strength; curvature stays linear.
    let knee = 0.05;
    let edge = 1.0 - exp(-gain * convex * convex / (convex + knee));
    let cavity = 1.0 - exp(-gain * concave * concave / (concave + knee));
    let bend = gain * (convex - concave);
    let curvature = 0.5 + 0.5 * sign(bend) * (1.0 - exp(-abs(bend)));

    // Thickness: rays into the mesh.
    var thick = 0.0;
    let th_n = max(P.counts.z, 1u);
    let inside = p - geo * eps;
    for (var i = 0u; i < th_n; i = i + 1u) {
        let d = cosine_dir(-n, r2(i + 977u, rot));
        let h = trace(inside, d, eps, P.dist.z, false);
        thick = thick + select(1.0, h.t / P.dist.z, h.t < MISS);
    }
    thick = thick / f32(th_n);

    // Sky light: soft shadows from a cone of directions.
    let l = normalize(P.sky.xyz);
    var vis = 0.0;
    let sky_n = max(P.counts.w, 1u);
    for (var i = 0u; i < sky_n; i = i + 1u) {
        let d = cone_dir(l, P.sky.w, r2(i + 1931u, rot));
        let h = trace(origin, d, eps, P.extra.y * 4.0, true);
        vis = vis + select(1.0, 0.0, h.t < MISS);
    }
    vis = vis / f32(sky_n);
    var lambert = max(dot(n, l), 0.0);
    if (stylized) { lambert = dot(n, l) * 0.5 + 0.5; }
    let sky = lambert * mix(0.35, 1.0, vis);

    textureStore(o_maps_a, xy, vec4<f32>(ao, edge, cavity, thick));
    textureStore(o_maps_b, xy, vec4<f32>(sky, curvature, 0.0, 0.0));
}

// ---------------------------------------------------------------- smooth

@compute @workgroup_size(8, 8)
fn soften(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<i32>(textureDimensions(s_src));
    let xy = vec2<i32>(id.xy);
    if (xy.x >= size.x || xy.y >= size.y) { return; }
    let center = textureLoad(s_src, xy, 0);
    let gp = textureLoad(g_pos, xy, 0);
    if (gp.w < 0.5 || P.extra.x <= 0.0) {
        textureStore(s_dst, xy, center);
        return;
    }
    let n = textureLoad(g_nrm, xy, 0).xyz;
    // A full 2D bilateral kernel (world distance + normal similarity). A separable
    // texture-axis blur smears along UV rows whose 3D direction changes per triangle and
    // leaves stair-step patches, so each pass (the host runs two, one per "axis") is an
    // isotropic 2D blur with sigma / sqrt(2): together they give sigma.
    let sigma = P.extra.x * 0.70710678;
    // Texel footprint in world units along each texture axis: the nearer covered
    // neighbour (the other may sit across a UV seam).
    var foot = vec2<f32>(1e30);
    for (var a = 0; a < 2; a = a + 1) {
        let axis = select(vec2<i32>(1, 0), vec2<i32>(0, 1), a == 1);
        for (var sgn = -1; sgn <= 1; sgn = sgn + 2) {
            let nb = clamp(xy + axis * sgn, vec2<i32>(0), size - 1);
            let nbp = textureLoad(g_pos, nb, 0);
            if (nbp.w > 0.5 && any(nb != xy)) { foot[a] = min(foot[a], distance(nbp.xyz, gp.xyz)); }
        }
    }
    if (foot.x > 1e29) { foot.x = foot.y; }
    if (foot.y > 1e29) { foot.y = foot.x; }
    if (foot.x > 1e29) {
        textureStore(s_dst, xy, center);
        return;
    }
    let radius = 5;
    let step = max(vec2<f32>(1.0), vec2<f32>(sigma * 2.0) / max(foot * f32(radius), vec2<f32>(1e-9)));
    var sum = center;
    var weight = 1.0;
    for (var j = -radius; j <= radius; j = j + 1) {
        for (var i = -radius; i <= radius; i = i + 1) {
            if (i == 0 && j == 0) { continue; }
            let q = clamp(xy + vec2<i32>(round(vec2<f32>(f32(i), f32(j)) * step)), vec2<i32>(0), size - 1);
            let qp = textureLoad(g_pos, q, 0);
            if (qp.w < 0.5) { continue; }
            let qn = textureLoad(g_nrm, q, 0).xyz;
            let d = distance(qp.xyz, gp.xyz);
            let wgt = exp(-d * d / (2.0 * sigma * sigma)) * pow(max(dot(qn, n), 0.0), 4.0);
            sum = sum + textureLoad(s_src, q, 0) * wgt;
            weight = weight + wgt;
        }
    }
    var out = sum / weight;
    // Edges / cavity (maps a: y, z) and curvature (maps b: y) are noise-free already;
    // a texture-space blur cannot cross UV seams and would leave steps along them.
    if (P.extra.w < 0.5) {
        out = vec4<f32>(out.x, center.y, center.z, out.w);
    } else {
        out = vec4<f32>(out.x, center.y, out.z, out.w);
    }
    textureStore(s_dst, xy, out);
}
