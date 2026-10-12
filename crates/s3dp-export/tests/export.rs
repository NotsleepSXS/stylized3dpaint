use std::path::PathBuf;

use s3dp_core::{BakedMap, ChannelId};
use s3dp_export::*;

const N: u32 = 64;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("s3dp-export-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn preset(name: &str) -> ExportPreset {
    builtin_presets().into_iter().find(|p| p.name.starts_with(name)).unwrap()
}

fn settings(dir: PathBuf) -> ExportSettings {
    ExportSettings { output_dir: dir, mesh_name: "Hero".into(), ..Default::default() }
}

/// Circle island in the middle, background uncovered.
fn circle_coverage() -> Vec<bool> {
    (0..N * N)
        .map(|i| {
            let (x, y) = ((i % N) as f32 + 0.5 - 32.0, (i / N) as f32 + 0.5 - 32.0);
            x * x + y * y < 20.0 * 20.0
        })
        .collect()
}

fn body() -> ExportSource {
    ExportSource::new("Body", N, N)
        .fill_channel(ChannelId::BaseColor, [0.5, 0.25, 0.1, 1.0])
        .fill_channel(ChannelId::Roughness, [0.3, 0.0, 0.0, 1.0])
        .fill_channel(ChannelId::Metallic, [0.6, 0.0, 0.0, 1.0])
        .fill_channel(ChannelId::Normal, [0.5, 0.7, 1.0, 1.0])
        .fill_channel(ChannelId::Emissive, [0.0, 0.0, 0.0, 1.0])
        .fill_baked(BakedMap::AmbientOcclusion, [0.8, 0.8, 0.8, 1.0])
        .fill_baked(BakedMap::Normal, [0.5, 0.5, 1.0, 1.0])
        .with_coverage(circle_coverage())
}

fn read_rgba(path: &std::path::Path) -> image::Rgba32FImage {
    image::open(path).unwrap().to_rgba32f()
}

#[test]
fn unreal_names_and_orm_values() {
    let dir = tmp("unreal");
    let rep = export(&[body()], &preset("Unreal Engine (Packed"), &settings(dir.clone())).unwrap();
    let mut names: Vec<String> =
        rep.files.iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["T_Body_BaseColor.png", "T_Body_Normal.png", "T_Body_ORM.png"]);
    let mut on_disk: Vec<String> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    on_disk.sort();
    assert_eq!(on_disk, names);
    assert!(rep.skipped.iter().any(|s| s.1 == "Emissive"));

    let orm = read_rgba(&dir.join("T_Body_ORM.png"));
    for (x, y) in [(32, 32), (20, 30), (0, 0), (63, 63)] {
        let p = orm.get_pixel(x, y).0;
        let tol = 1.01 / 255.0;
        assert!((p[0] - 0.8).abs() < tol, "AO {p:?}");
        assert!((p[1] - 0.3).abs() < tol, "Roughness {p:?}");
        assert!((p[2] - 0.6).abs() < tol, "Metallic {p:?}");
    }
    // Base color is sRGB-encoded, no alpha (opacity unused).
    let bc = image::open(dir.join("T_Body_BaseColor.png")).unwrap();
    assert_eq!(bc.color(), image::ColorType::Rgb8);
    let p = bc.to_rgb32f().get_pixel(32, 32).0;
    let want = s3dp_core::document::linear_to_srgb(0.5);
    assert!((p[0] - want).abs() < 1.01 / 255.0, "{p:?} vs {want}");
}

#[test]
fn opacity_flag_adds_alpha() {
    let dir = tmp("opacity");
    let mut s = body().fill_channel(ChannelId::Opacity, [0.25, 0.0, 0.0, 1.0]);
    s.uses_opacity = s.detect_uses_opacity();
    assert!(s.uses_opacity);
    export(&[s], &preset("Unreal Engine (Packed"), &settings(dir.clone())).unwrap();
    let bc = image::open(dir.join("T_Body_BaseColor.png")).unwrap();
    assert_eq!(bc.color(), image::ColorType::Rgba8);
    assert!((bc.to_rgba32f().get_pixel(32, 32).0[3] - 0.25).abs() < 1.01 / 255.0);
}

fn normal_green(preset_name: &str, file: &str) -> f32 {
    let dir = tmp(&preset_name.replace(['/', ' ', '(', ')', '.'], "_"));
    let mut st = settings(dir.clone());
    st.dither = Dither::None;
    export(&[body()], &preset(preset_name), &st).unwrap();
    read_rgba(&dir.join(file)).get_pixel(32, 32).0[1]
}

#[test]
fn directx_vs_opengl_normal_green() {
    let ue = normal_green("Unreal Engine (Packed", "T_Body_Normal.png");
    // Expected OpenGL green: normalize((0, 0.4, 1)) encoded.
    let n = [0.0f32, 0.4, 1.0];
    let l = (n[1] * n[1] + 1.0).sqrt();
    let gl_expected = n[1] / l * 0.5 + 0.5;
    for (p, f) in [
        ("Unity URP", "Hero_Body_Normal.png"),
        ("Godot", "Hero_Body_Normal.png"),
        ("glTF", "Hero_Body_Normal.png"),
    ] {
        let gl = normal_green(p, f);
        assert!((gl - gl_expected).abs() < 1.01 / 255.0, "{p}: {gl} vs {gl_expected}");
        assert!((gl + ue - 1.0).abs() < 1.01 / 255.0, "{p}: gl {gl} ue {ue}");
    }
    assert!(ue < 0.4);
}

#[test]
fn rnm_combines_detail_over_baked() {
    // Baked tilted normal + flat detail → baked normal unchanged.
    let dir = tmp("rnm");
    let s = ExportSource::new("Set", 4, 4)
        .fill_baked(BakedMap::Normal, [0.7, 0.5, 0.9, 1.0])
        .fill_channel(ChannelId::Normal, [0.5, 0.5, 1.0, 1.0]);
    let p = preset("glTF");
    let o = p.outputs.iter().find(|o| o.suffix == "Normal").unwrap();
    let img = render_output(&s, &p, o, &settings(dir));
    let v = [0.4f32, 0.0, 0.8];
    let l = (v[0] * v[0] + v[2] * v[2]).sqrt();
    assert!((img.data[0] - (v[0] / l * 0.5 + 0.5)).abs() < 1e-4);
    assert!((img.data[1] - 0.5).abs() < 1e-4);
}

#[test]
fn hdrp_maskmap_alpha_is_smoothness() {
    let dir = tmp("hdrp");
    let rep = export(&[body()], &preset("Unity HDRP"), &settings(dir.clone())).unwrap();
    let mask = rep.files.iter().find(|f| f.suffix == "MaskMap").unwrap();
    let p = read_rgba(&mask.path).get_pixel(32, 32).0;
    let tol = 1.01 / 255.0;
    assert!((p[3] - 0.7).abs() < tol, "A {p:?}");
    assert!((p[0] - 0.6).abs() < tol, "R metallic {p:?}");
    assert!((p[1] - 0.8).abs() < tol, "G AO {p:?}");
    assert!((p[2] - 1.0).abs() < tol, "B detail {p:?}");
}

#[test]
fn custom_preset_json_roundtrip_identical_bytes() {
    let mut p = preset("Custom");
    p.name = "My Packed".into();
    p.normal = NormalFormat::DirectX;
    p.naming = Some("$project_$textureSet_$suffix_$res".into());
    let mut packed = OutputTexture::new(
        "Mask",
        OutputKind::Rgba,
        false,
        [
            Some(Slot::new(Source::Baked { map: BakedMap::AmbientOcclusion, component: 0 })),
            Some(Slot { source: Source::Channel { channel: ChannelId::Roughness, component: 0 }, invert: true, remap_min: 0.1, remap_max: 0.9, gamma: 2.2 }),
            Some(Slot::new(Source::NormalCombined { component: 1 })),
            Some(Slot::new(Source::Constant { value: 0.333 })),
        ],
    );
    packed.format = Some(FileFormat::Png16);
    p.outputs.push(packed);
    p.outputs.push(OutputTexture::new(
        "Spec",
        OutputKind::Rgb,
        true,
        [0, 1, 2].map(|c| Some(Slot::new(Source::PbrSpecular { component: c })))
            .into_iter().chain([None]).collect::<Vec<_>>().try_into().unwrap(),
    ));

    let dir = tmp("custom");
    let json = dir.join("preset.json");
    save_preset(&json, &p).unwrap();
    let loaded = load_preset(&json).unwrap();
    assert_eq!(loaded, p);

    let a = export(&[body()], &p, &settings(dir.join("a"))).unwrap();
    let b = export(&[body()], &loaded, &settings(dir.join("b"))).unwrap();
    assert_eq!(a.files.len(), 2);
    for (fa, fb) in a.files.iter().zip(&b.files) {
        assert_eq!(fa.path.file_name(), fb.path.file_name());
        assert_eq!(std::fs::read(&fa.path).unwrap(), std::fs::read(&fb.path).unwrap());
    }
    assert!(a.files[0].path.ends_with("Project_Body_Mask_64.png"));
}

#[test]
fn infinite_padding_fills_everything_without_halo() {
    let dir = tmp("padding");
    let cov = circle_coverage();
    // Gradient inside the island so "nearest" matters; zeros elsewhere.
    let mut base = vec![0.0f32; (N * N * 4) as usize];
    for i in 0..(N * N) as usize {
        if cov[i] {
            let x = (i as u32 % N) as f32 / N as f32;
            base[i * 4..i * 4 + 4].copy_from_slice(&[0.6 + 0.2 * x, 0.5, 0.4, 1.0]);
        }
    }
    let s = ExportSource::new("Body", N, N).with_channel(ChannelId::BaseColor, base).with_coverage(cov.clone());
    let p = preset("Sketchfab");
    let mut st = settings(dir.clone());
    st.dither = Dither::None;
    let rep = export(&[s], &p, &st).unwrap();
    let f = rep.files.iter().find(|f| f.suffix == "BaseColor").unwrap();
    let img = read_rgba(&f.path);
    for y in 0..N {
        for x in 0..N {
            let px = img.get_pixel(x, y).0;
            assert!(px[1] > 0.5, "texel ({x},{y}) kept background {px:?}");
        }
    }
    // 1/16 mip: 4×4 texels per 16×16 block; border blocks must match inside color (no dark halo).
    let inside_g = img.get_pixel(32, 32).0[1];
    for by in 0..4 {
        for bx in 0..4 {
            let mut sum = 0.0;
            for y in 0..16 {
                for x in 0..16 {
                    sum += img.get_pixel(bx * 16 + x, by * 16 + y).0[1];
                }
            }
            let avg = sum / 256.0;
            assert!((avg - inside_g).abs() < 2.0 / 255.0, "block ({bx},{by}) avg {avg} vs {inside_g}");
        }
    }
}

#[test]
fn transparent_and_pixel_padding() {
    let p = preset("glTF");
    let o = p.outputs.iter().find(|o| o.suffix == "Occlusion").unwrap();
    let s = body();
    let mut st = settings(tmp("transparent"));
    st.padding = Padding::Transparent;
    let img = render_output(&s, &p, o, &st);
    assert_eq!(img.channels, 2);
    assert_eq!(img.data[1], 0.0); // corner: alpha 0
    assert!((img.data[0] - 0.8).abs() < 1e-6); // color still dilated
    st.padding = Padding::Pixels(2);
    let img = render_output(&s, &p, o, &st);
    assert_eq!(img.channels, 1);
    assert_eq!(img.data[0], 0.0); // far corner untouched
    // Texel 1 px left of the island edge on the middle row is padded.
    let row = 32usize;
    let first = (0..N as usize).find(|&x| s.coverage[row * N as usize + x]).unwrap();
    assert!((img.data[row * N as usize + first - 1] - 0.8).abs() < 1e-6);
    assert_eq!(img.data[row * N as usize + first - 4], 0.0);
}

#[test]
fn png16_precision() {
    let dir = tmp("png16");
    let mut h = vec![0.0f32; (N * N * 4) as usize];
    for i in 0..(N * N) as usize {
        h[i * 4] = i as f32 / (N * N - 1) as f32 * 0.987 + 0.0013;
    }
    let s = ExportSource::new("Body", N, N).with_channel(ChannelId::Height, h.clone());
    let mut p = preset("Unreal Engine (Packed");
    p.outputs.retain(|o| o.suffix == "Height");
    p.outputs[0].enabled = true;
    let mut st = settings(dir.clone());
    st.format = FileFormat::Png16;
    let rep = export(&[s], &p, &st).unwrap();
    let img = image::open(&rep.files[0].path).unwrap();
    assert_eq!(img.color(), image::ColorType::L16);
    let l = img.to_luma16();
    let mut max_err = 0.0f32;
    for i in 0..(N * N) as usize {
        let v = l.as_raw()[i] as f32 / 65535.0;
        max_err = max_err.max((v - h[i * 4]).abs());
    }
    assert!(max_err < 1.0 / 30000.0, "max err {max_err}");
}

#[test]
fn resolution_override_and_other_formats() {
    let dir = tmp("formats");
    for fmt in FileFormat::ALL {
        let mut st = settings(dir.join(fmt.extension()).join(format!("{fmt:?}")));
        st.format = fmt;
        st.resolution_override.insert("Body".into(), [32, 32]);
        let rep = export(&[body()], &preset("Unity URP"), &st).unwrap();
        assert!(!rep.files.is_empty());
        for f in &rep.files {
            assert_eq!((f.width, f.height), (32, 32));
            assert!(f.path.exists(), "{}", f.path.display());
            if fmt != FileFormat::Exr16 {
                let img = image::open(&f.path);
                if let Ok(img) = img {
                    assert_eq!(img.width(), 32);
                }
            }
        }
        if fmt == FileFormat::Jpg8 {
            assert!(rep.warnings.iter().any(|w| w.kind == WarningKind::LossyFormat));
        }
    }
}

#[test]
fn validate_reports_problems() {
    let mut p = preset("Custom");
    p.outputs.push(OutputTexture::new(
        "Bad",
        OutputKind::Rgb,
        false,
        [Some(Slot::new(Source::Channel { channel: ChannelId::BaseColor, component: 0 })), None, Some(Slot::new(Source::Channel { channel: ChannelId::User3, component: 0 })), None],
    ));
    let w = validate(&[body()], &p, &ExportSettings::default());
    let kinds: Vec<_> = w.iter().map(|w| w.kind).collect();
    assert!(kinds.contains(&WarningKind::EmptySlot));
    assert!(kinds.contains(&WarningKind::SrgbDataInLinear));
    assert!(kinds.contains(&WarningKind::MissingChannel));
    // Built-ins are clean for a complete source.
    for p in builtin_presets().iter().filter(|p| p.name != "Custom") {
        let w = validate(&[body()], p, &ExportSettings::default());
        assert!(
            w.iter().all(|w| matches!(w.kind, WarningKind::MissingChannel | WarningKind::MissingBakedMap)),
            "{}: {w:?}",
            p.name
        );
    }
}

#[test]
#[ignore = "perf: run with --release --ignored"]
fn padding_4k_perf() {
    let n = 4096usize;
    let cov: Vec<bool> = (0..n * n).map(|i| (i % n) % 512 < 300 && (i / n) % 512 < 300).collect();
    let t = std::time::Instant::now();
    let near = s3dp_export::__bench::nearest_map(n, n, &cov);
    let dt = t.elapsed();
    assert!(near.iter().all(|&j| j != u32::MAX));
    eprintln!("4096² nearest map: {dt:?}");
    let s = ExportSource::new("Big", n as u32, n as u32)
        .fill_channel(ChannelId::Roughness, [0.5, 0.0, 0.0, 1.0])
        .with_coverage(cov);
    let p = preset("Unreal Engine (Packed");
    let o = p.outputs.iter().find(|o| o.suffix == "ORM").unwrap();
    let t = std::time::Instant::now();
    let _ = render_output(&s, &p, o, &ExportSettings::default());
    eprintln!("4096² ORM render+pad: {:?}", t.elapsed());
}
