struct Globals {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x: shading mode, y: checker scale, z: UV layout (1 = draw the mesh flat in UV space)
    params: vec4<f32>,
    inv_view_proj: mat4x4<f32>,
    // Irradiance SH (rgb), pre-convolved: eval = diffuse light for a white Lambert surface.
    sh: array<vec4<f32>, 9>,
    // x: rotation (rad), y: exposure (linear), z: max lod (roughness 1), w: env lighting on
    env: vec4<f32>,
    // x: background on, y: background lod
    env2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var env_tex: texture_2d<f32>;   // equirect, mip k = GGX roughness k / max
@group(0) @binding(2) var env_samp: sampler;

const PI: f32 = 3.14159265;

/// World direction -> environment texture direction (rotated about Y).
fn env_dir(d: vec3<f32>) -> vec3<f32> {
    let a = g.env.x;
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(c * d.x + s * d.z, d.y, -s * d.x + c * d.z);
}

fn env_uv(d: vec3<f32>) -> vec2<f32> {
    let n = normalize(d);
    return vec2<f32>(0.5 + atan2(n.x, -n.z) / (2.0 * PI), acos(clamp(n.y, -1.0, 1.0)) / PI);
}

fn env_sample(d: vec3<f32>, lod: f32) -> vec3<f32> {
    return textureSampleLevel(env_tex, env_samp, env_uv(env_dir(d)), lod).rgb;
}

fn sh_irradiance(n0: vec3<f32>) -> vec3<f32> {
    let n = env_dir(n0);
    var r = g.sh[0].rgb * 0.282095;
    r += g.sh[1].rgb * 0.488603 * n.y + g.sh[2].rgb * 0.488603 * n.z + g.sh[3].rgb * 0.488603 * n.x;
    r += g.sh[4].rgb * 1.092548 * n.x * n.y + g.sh[5].rgb * 1.092548 * n.y * n.z;
    r += g.sh[6].rgb * 0.315392 * (3.0 * n.z * n.z - 1.0) + g.sh[7].rgb * 1.092548 * n.x * n.z;
    r += g.sh[8].rgb * 0.546274 * (n.x * n.x - n.y * n.y);
    return max(r, vec3<f32>(0.0));
}

/// Split-sum environment BRDF, analytic fit (Karis, "Physically Based Shading on Mobile").
fn env_brdf(f0: vec3<f32>, rough: f32, nv: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = rough * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

/// Image-based lighting: SH diffuse + prefiltered specular, ambient occlusion on both.
fn lit_ibl(n: vec3<f32>, v: vec3<f32>, albedo: vec3<f32>, rough_in: f32, metal: f32, ao: f32) -> vec3<f32> {
    let rough = clamp(rough_in, 0.02, 1.0);
    let nv = max(dot(n, v), 1e-4);
    let f0 = mix(vec3<f32>(0.04), albedo, metal);
    let diffuse = albedo * (1.0 - metal) * sh_irradiance(n) * ao;
    let r = reflect(-v, n);
    let spec_occ = clamp(pow(nv + ao, exp2(-16.0 * rough - 1.0)) - 1.0 + ao, 0.0, 1.0);
    let spec = env_sample(r, rough * g.env.z) * env_brdf(f0, rough, nv) * spec_occ;
    return (diffuse + spec) * g.env.y;
}

/// ACES filmic tone curve (Narkowicz fit).
fn tonemap(x: vec3<f32>) -> vec3<f32> {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Background: fullscreen triangle showing the environment (blurred by a mip level).
struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_bg(@builtin(vertex_index) i: u32) -> BgOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: BgOut;
    o.clip = vec4<f32>(p, 1.0, 1.0);
    o.ndc = p;
    return o;
}

@fragment
fn fs_bg(i: BgOut) -> @location(0) vec4<f32> {
    let far = g.inv_view_proj * vec4<f32>(i.ndc, 1.0, 1.0);
    let d = normalize(far.xyz / far.w - g.eye.xyz);
    return vec4<f32>(tonemap(env_sample(d, g.env2.y) * g.env.y), 1.0);
}
@group(1) @binding(0) var base_color_tex: texture_2d<f32>;   // sRGB texture: samples are linear
@group(1) @binding(1) var roughness_tex: texture_2d<f32>;
@group(1) @binding(2) var metallic_tex: texture_2d<f32>;
@group(1) @binding(3) var normal_tex: texture_2d<f32>;       // layer normal channel, encoded 0..1
@group(1) @binding(4) var height_tex: texture_2d<f32>;
@group(1) @binding(5) var ao_channel_tex: texture_2d<f32>;
@group(1) @binding(6) var emissive_tex: texture_2d<f32>;     // sRGB texture
@group(1) @binding(7) var ao_tex: texture_2d<f32>;           // baked AO
@group(1) @binding(8) var normal_map_tex: texture_2d<f32>;   // baked tangent-space normal
@group(1) @binding(9) var inspect_tex: texture_2d<f32>;
@group(1) @binding(10) var paint_sampler: sampler;

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
    if (g.params.z > 0.5) {
        // Flat texture view: UV (0,0) top-left -> clip (-1, 1).
        o.clip = vec4<f32>(v.uv.x * 2.0 - 1.0, 1.0 - v.uv.y * 2.0, 0.5, 1.0);
    }
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

fn rnm(b: vec3<f32>, s: vec3<f32>) -> vec3<f32> {
    let t = b * vec3<f32>(2.0, 2.0, 2.0) + vec3<f32>(-1.0, -1.0, 0.0);
    let u = s * vec3<f32>(-2.0, -2.0, 2.0) + vec3<f32>(1.0, 1.0, -1.0);
    return normalize(t * dot(t, u) - u * t.z) * 0.5 + 0.5;
}

@fragment
fn fs_main(i: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(i.normal);
    let v = normalize(g.eye.xyz - i.world);
    let mode = u32(g.params.x);

    // Baked normal combined with the painted normal channel (RNM), MikkTSpace / OpenGL +Y.
    let baked_n = textureSample(normal_map_tex, paint_sampler, i.uv).xyz;
    let layer_n = textureSample(normal_tex, paint_sampler, i.uv).xyz;
    let nm = rnm(baked_n, layer_n) * 2.0 - 1.0;
    var t = i.tangent.xyz - n * dot(i.tangent.xyz, n);
    if (dot(t, t) > 1e-12) {
        t = normalize(t);
        let b = select(1.0, -1.0, i.tangent.w < 0.0) * cross(n, t);
        n = normalize(t * nm.x + b * nm.y + n * nm.z);
    }
    if (!front) { n = -n; }
    let ao = textureSample(ao_tex, paint_sampler, i.uv).r * textureSample(ao_channel_tex, paint_sampler, i.uv).r;

    let albedo = textureSample(base_color_tex, paint_sampler, i.uv).rgb;
    let rough = textureSample(roughness_tex, paint_sampler, i.uv).r;
    let metal = textureSample(metallic_tex, paint_sampler, i.uv).r;
    let height = textureSample(height_tex, paint_sampler, i.uv).r;
    let emissive = textureSample(emissive_tex, paint_sampler, i.uv).rgb;
    let insp = textureSample(inspect_tex, paint_sampler, i.uv);

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
        case 50u: { col = srgb_to_linear(vec3<f32>(insp.r)); }
        case 51u: { col = srgb_to_linear(insp.rgb); }
        case 52u: { col = insp.rgb; }
        case 8u: {
            // Toon: three-tone ramp with soft steps, cool shadows and a rim light.
            let key = normalize(vec3<f32>(0.5, 0.8, 0.4));
            let d = dot(bn, key) * 0.5 + 0.5;
            let ramp = smoothstep(0.38, 0.42, d) * 0.45 + smoothstep(0.68, 0.72, d) * 0.35 + 0.2;
            let light = mix(vec3<f32>(0.55, 0.6, 0.85), vec3<f32>(1.0, 0.97, 0.9), ramp);
            let rim = smoothstep(0.62, 0.68, 1.0 - max(dot(bn, v), 0.0)) * 0.35;
            col = albedo * light * mix(1.0, ao, 0.5) + rim * albedo + emissive;
        }
        default: {
            if (g.env.w > 0.5) {
                col = tonemap(lit_ibl(bn, v, albedo, rough, metal, mix(1.0, ao, 0.9))) + emissive;
            } else {
                col = lit(bn, v, albedo, rough, metal) * mix(1.0, ao, 0.8) + emissive;
            }
        }
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

// Overlays for the Polygon Fill tool: wireframe lines and the highlighted triangles, in 3D
// or flat in UV space (params.z), nudged towards the camera to sit on the surface.
@vertex
fn vs_overlay(v: VsIn) -> @builtin(position) vec4<f32> {
    if (g.params.z > 0.5) {
        return vec4<f32>(v.uv.x * 2.0 - 1.0, 1.0 - v.uv.y * 2.0, 0.4, 1.0);
    }
    var c = g.view_proj * vec4<f32>(v.position, 1.0);
    c.z = c.z - 3e-4 * c.w;
    return c;
}

@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    return vec4<f32>(0.05, 0.05, 0.06, 0.55);
}

@fragment
fn fs_highlight() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.45, 0.1, 0.38);
}

// Per-group tint (parts / UV islands) for the Polygon Fill tool.
struct TintOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_tint(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> TintOut {
    var o: TintOut;
    if (g.params.z > 0.5) {
        o.clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.45, 1.0);
    } else {
        o.clip = g.view_proj * vec4<f32>(position, 1.0);
        o.clip.z = o.clip.z - 2e-4 * o.clip.w;
    }
    o.color = color;
    return o;
}

@fragment
fn fs_tint(i: TintOut) -> @location(0) vec4<f32> {
    return vec4<f32>(i.color.rgb, 0.22);
}
