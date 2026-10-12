# Audit (2026-10-12, before the spec "ТЗ: Stylize 3D Painter")

Build: `cargo build --release` OK, `cargo test` 18/18 green (RTX 4080, Vulkan, Linux/Wayland).

## Stack

| Area | Tech | Files |
|---|---|---|
| Language / UI | Rust 2024, egui/eframe 0.36, egui-phosphor icons | `crates/s3dp-app` |
| GPU | wgpu 30 (Vulkan), WGSL shaders | `crates/s3dp-render` |
| Core data | glam, serde, gltf/tobj/ufbx import, bevy_mikktspace tangents, CPU BVH | `crates/s3dp-core` |
| Project file | `.s3dp` = zip: `project.json` (format 2, migrates 1) + `mesh.bin` + PNG layers + baked maps | `crates/s3dp-app/src/project.rs` |

## What exists and works

- Import FBX / OBJ / glTF(GLB), material slots → texture sets, MikkTSpace tangents, vertex welding.
- Viewport: orbit camera, MSAA, modes Lit / Toon / Unlit / UV / Normals / per channel / per baked map;
  redraws only when something changed.
- Painting in UV space with screen-space dabs, occlusion test, seam-free dilation, hardness / opacity /
  flow, eraser, eyedropper (whole material), undo/redo (snapshots, 40 steps).
- Layer stack: Paint, Fill, Stylize filter layer; masks (painted), 18 Photoshop blend modes, opacity,
  per-layer channel toggles. Channels: Base Color, Roughness, Metallic, Height, Emissive.
- Mask effects: generators (baked maps, height gradient, 3D noise Perlin/Cellular/Strokes),
  Warp / Slope Blur / Directional, Blur, Levels. Noise in world space → no UV seams. Effect results cached
  per layer, recompute throttled to 10/s while dragging.
- GPU baker (BVH + compute): AO, Curvature, Edges, Cavity, Thickness, Sky Light, Normal (from high-poly),
  World Normal, Position; Realistic / Stylized modes; progressive with progress bar.
- PNG export of all channels per texture set (no presets, no packing).
- Photoshop-style UI: tool bar, options bar, Color / Material / Properties / Layers dock, smart layers.

## What is broken / weak

- **Stylized bake artifacts**: Edges / Cavity light up on every polygon edge of smooth meshes (KNOWN_ISSUES.md).
- No ID map bake.
- Blending happens in sRGB on 8-bit layer textures (spec wants linear, ≥16-bit internal).
- Effects exist only on masks, not on content; each effect is hand-written in Rust + WGSL + UI (spec wants a
  declarative effect engine with generated UI).
- No folders, effect layers inside folders, anchors, instances, per-channel blend modes.
- Channels missing: Normal (layer), AO, Opacity, User0–7.
- Export: no window, presets, channel packing, DirectX normals, padding options.
- No localization (UI is English), no autosave, logs only to stderr, undo 40 steps.

## Reuse plan

Keep: import, BVH baker, mesh maps, viewport, painting (dab / stroke-mask pipeline), dilation, thumbnails,
undo-by-snapshot model, project zip container (bump format + migrate), theme / Photoshop UI pieces,
noise + warp WGSL (moved into the shared effect prelude).

Replace: `Painter` compositing (flat list → tree with folders, effect stacks on content and mask, linear
16-bit float buffers, per-channel blend), hand-written mask effects (→ declarative effects), Stylize layer
(→ Stylize Master effect).
