use std::path::Path;

/// All three formats of the same Blender scene should import to the same geometry.
#[test]
fn formats_agree() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/test");
    let mut results = Vec::new();
    for ext in ["glb", "fbx", "obj"] {
        let mesh = s3dp_core::import::load(&dir.join(format!("monkey_crate.{ext}"))).unwrap();
        let b = mesh.bounds();
        println!("{ext}: tris={} parts={:?} min={:?} max={:?}", mesh.triangle_count(),
            mesh.parts.iter().map(|p| &p.name).collect::<Vec<_>>(), b.min, b.max);
        assert!(mesh.has_uvs());
        results.push((mesh.triangle_count(), b));
    }
    for (tris, b) in &results[1..] {
        assert_eq!(*tris, results[0].0);
        assert!((b.min - results[0].1.min).length() < 1e-3 && (b.max - results[0].1.max).length() < 1e-3);
    }
}
