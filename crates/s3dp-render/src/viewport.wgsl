struct Globals {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x: shading mode, y: checker scale
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var base_color_tex: texture_2d<f32>;
@group(1) @binding(1) var roughness_tex: texture_2d<f32>;
@group(1) @binding(2) var metallic_tex: texture_2d<f32>;
@group(1) @binding(3) var height_tex: texture_2d<f32>;
@group(1) @binding(4) var emissive_tex: texture_2d<f32>;
@group(1) @binding(5) var ao_tex: texture_2d<f32>;
@group(1) @binding(6) var curvature_tex: texture_2d<f32>;
@group(1) @binding(7) var edges_tex: texture_2d<f32>;
@group(1) @binding(8) var cavity_tex: texture_2d<f32>;
@group(1) @binding(9) var thickness_tex: texture_2d<f32>;
@group(1) @binding(10) var sky_tex: texture_2d<f32>;
@group(1) @binding(11) var normal_map_tex: texture_2d<f32>;
@group(1) @binding(12) var world_normal_tex: texture_2d<f32>;
@group(1) @binding(13) var position_tex: texture_2d<f32>;
@group(1) @binding(14) var paint_sampler: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = g.view_proj * vec4<f32>(v.position, 1.0);
    o.world = v.position;
    o.normal = v.normal;
    o.uv = v.uv;
    o.tangent = v.tangent;
    return o;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn lit(n: vec3<f32>, v: vec3<f32>, albedo: vec3<f32>, rough: f32, metal: f32) -> vec3<f32> {
    let key = normalize(vec3<f32>(0.5, 0.8, 0.4));
    let fill = normalize(vec3<f32>(-0.6, 0.2, -0.5));
    let wrap = max(dot(n, key) * 0.5 + 0.5, 0.0);
    let sky = mix(vec3<f32>(0.25, 0.22, 0.2), vec3<f32>(0.55, 0.6, 0.7), n.y * 0.5 + 0.5);
    let diffuse = albedo * (1.0 - metal);
    let spec_col = mix(vec3<f32>(0.04), albedo, metal);
    let h = normalize(key + v);
    let gloss = 1.0 - clamp(rough, 0.02, 1.0);
    let power = exp2(gloss * 10.0 + 1.0);
    let spec = pow(max(dot(n, h), 0.0), power) * (power + 8.0) / 25.0 * max(dot(n, key), 0.0);
    let fres = spec_col + (1.0 - spec_col) * pow(1.0 - max(dot(n, v), 0.0), 5.0) * gloss;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0) * 0.12;
    let ambient_spec = fres * sky * 0.35 * (0.3 + 0.7 * gloss);
    return diffuse * (wrap * wrap * vec3<f32>(1.0, 0.96, 0.9) + sky * 0.35 + max(dot(n, fill), 0.0) * 0.15)
        + spec_col * spec + ambient_spec + rim * diffuse;
}

@fragment
fn fs_main(i: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(i.normal);
    let v = normalize(g.eye.xyz - i.world);
    let mode = u32(g.params.x);

    // Baked tangent-space normal map (MikkTSpace, OpenGL +Y).
    let nm = textureSample(normal_map_tex, paint_sampler, i.uv).xyz * 2.0 - 1.0;
    var t = i.tangent.xyz - n * dot(i.tangent.xyz, n);
    if (dot(t, t) > 1e-12) {
        t = normalize(t);
        let b = select(1.0, -1.0, i.tangent.w < 0.0) * cross(n, t);
        n = normalize(t * nm.x + b * nm.y + n * nm.z);
    }
    if (!front) { n = -n; }
    let ao = textureSample(ao_tex, paint_sampler, i.uv).r;

    let albedo = srgb_to_linear(textureSample(base_color_tex, paint_sampler, i.uv).rgb);
    let rough = textureSample(roughness_tex, paint_sampler, i.uv).r;
    let metal = textureSample(metallic_tex, paint_sampler, i.uv).r;
    let height = textureSample(height_tex, paint_sampler, i.uv).r;
    let emissive = srgb_to_linear(textureSample(emissive_tex, paint_sampler, i.uv).rgb);

    // Bump from height via screen-space derivatives.
    let dp_dx = dpdx(i.world);
    let dp_dy = dpdy(i.world);
    let dh_dx = dpdx(height);
    let dh_dy = dpdy(height);
    let r1 = cross(dp_dy, n);
    let r2 = cross(n, dp_dx);
    let det = dot(dp_dx, r1);
    let bump = 0.02 * length(g.eye.xyz - i.world);
    let grad = sign(det) * (dh_dx * r1 + dh_dy * r2) * bump;
    let bn = normalize(abs(det) * n - grad);

    var col: vec3<f32>;
    switch mode {
        case 1u: { col = albedo + emissive; }
        case 2u: {
            let c = floor(i.uv * g.params.y);
            let check = (i32(c.x) + i32(c.y)) & 1;
            col = lit(n, v, select(vec3<f32>(0.25, 0.45, 0.85), vec3<f32>(0.85), check == 0), 0.7, 0.0);
        }
        case 3u: { col = srgb_to_linear(bn * 0.5 + 0.5); }
        case 4u: { col = srgb_to_linear(vec3<f32>(rough)); }
        case 5u: { col = srgb_to_linear(vec3<f32>(metal)); }
        case 6u: { col = srgb_to_linear(vec3<f32>(height)); }
        case 7u: { col = emissive; }
        case 8u: {
            // Toon: three-tone ramp with soft steps, cool shadows and a rim light.
            let key = normalize(vec3<f32>(0.5, 0.8, 0.4));
            let d = dot(bn, key) * 0.5 + 0.5;
            let ramp = smoothstep(0.38, 0.42, d) * 0.45 + smoothstep(0.68, 0.72, d) * 0.35 + 0.2;
            let shadow = vec3<f32>(0.55, 0.6, 0.85);
            let light = mix(shadow, vec3<f32>(1.0, 0.97, 0.9), ramp);
            let rim = smoothstep(0.62, 0.68, 1.0 - max(dot(bn, v), 0.0)) * 0.35;
            col = albedo * light * mix(1.0, ao, 0.5) + rim * albedo + emissive;
        }
        case 100u: { col = srgb_to_linear(vec3<f32>(ao)); }
        case 101u: { col = srgb_to_linear(textureSample(curvature_tex, paint_sampler, i.uv).rrr); }
        case 102u: { col = srgb_to_linear(textureSample(edges_tex, paint_sampler, i.uv).rrr); }
        case 103u: { col = srgb_to_linear(textureSample(cavity_tex, paint_sampler, i.uv).rrr); }
        case 104u: { col = srgb_to_linear(textureSample(thickness_tex, paint_sampler, i.uv).rrr); }
        case 105u: { col = srgb_to_linear(textureSample(sky_tex, paint_sampler, i.uv).rrr); }
        case 106u: { col = srgb_to_linear(textureSample(normal_map_tex, paint_sampler, i.uv).rgb); }
        case 107u: { col = srgb_to_linear(textureSample(world_normal_tex, paint_sampler, i.uv).rgb); }
        case 108u: { col = srgb_to_linear(textureSample(position_tex, paint_sampler, i.uv).rgb); }
        default: { col = lit(bn, v, albedo, rough, metal) * mix(1.0, ao, 0.8) + emissive; }
    }
    if (!front) { col *= vec3<f32>(0.6, 0.35, 0.35); }
    return vec4<f32>(col, 1.0);
}

// Picking: writes (u, v, texture set + 1) of the surface under each pixel.
struct PickOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) set_index: u32,
};

@vertex
fn vs_pick(v: VsIn, @builtin(instance_index) set_index: u32) -> PickOut {
    var o: PickOut;
    o.clip = g.view_proj * vec4<f32>(v.position, 1.0);
    o.uv = v.uv;
    o.set_index = set_index;
    return o;
}

@fragment
fn fs_pick(i: PickOut) -> @location(0) vec4<f32> {
    return vec4<f32>(i.uv, f32(i.set_index + 1u), 1.0);
}
