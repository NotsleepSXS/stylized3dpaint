//! Embeds every built-in effect (`effects/<id>.json` + `effects/<id>.wgsl`) into the binary.

use std::fmt::Write;

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("effects");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .expect("effects directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    ids.sort();
    let mut out = String::from("pub static BUILTIN_EFFECTS: &[(&str, &str, &str)] = &[\n");
    for id in ids {
        let json = dir.join(format!("{id}.json"));
        let wgsl = dir.join(format!("{id}.wgsl"));
        println!("cargo:rerun-if-changed={}", json.display());
        println!("cargo:rerun-if-changed={}", wgsl.display());
        assert!(wgsl.exists(), "effect {id} has no {id}.wgsl");
        writeln!(out, "    ({id:?}, include_str!({:?}), include_str!({:?})),", json.display().to_string(), wgsl.display().to_string()).unwrap();
    }
    out.push_str("];\n");
    std::fs::write(std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("builtin_effects.rs"), out).unwrap();
}
