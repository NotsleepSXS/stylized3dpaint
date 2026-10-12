# Stylized3DPaint

Open-source 3D texture painter focused on stylized / hand-painted texturing.
Written in Rust with wgpu and egui.

**Status:** early development (pre-release). Builds and runs on Linux; Windows / macOS untested.

| Lit | Toon |
|-----|------|
| ![Lit viewport](docs/images/showcase_lit.png) | ![Toon viewport](docs/images/showcase_toon.png) |

*Test model with a Stylized bake and an effect stack: metal edge wear, edge highlight, cavity
darken, hue shift by light — no hand painting.*

## Build

Needs a recent stable Rust (edition 2024) and a Vulkan / Metal / DX12 capable GPU.

```sh
git clone https://github.com/noslivideo/stylized3dpaint
cd stylized3dpaint
cargo run --release -- path/to/model.fbx
```

Supported model formats: glTF/GLB, FBX, OBJ.

## Controls

| Action | Input |
|--------|-------|
| Paint / erase / eyedropper | B / E / I, then LMB |
| Brush size | [ and ] |
| Swap / default colors | X / D |
| Orbit | Alt+LMB or MMB |
| Pan | RMB, Alt+MMB or Shift+MMB |
| Zoom | Wheel or Alt+RMB |
| Frame model | F |
| Viewport mode | F1–F9 (Lit, Toon, Unlit, UV, Normals, Roughness, Metallic, Height, Emissive) |
| Undo / redo | Ctrl+Z / Ctrl+Shift+Z |
| New layer / duplicate / delete | Ctrl+Shift+N / Ctrl+J / Del |
| New / open / save / export | Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+E |
| Bake mesh maps | Ctrl+B |
| Pen (Wayland) | tip paints with pressure, eraser end erases, lower button orbits, upper button = RMB |

## Features

- Texture-space painting with occlusion test and seam-free UV borders
- Layer stack shared by all texture sets: paint and fill layers, masks, 18 Photoshop blend modes, opacity
- Channels: Base Color, Roughness, Metallic, Height (shown as bump), Emissive
- Eyedropper picks the whole material under the cursor
- GPU ray-traced baking (BVH + compute shaders), with or without a high-poly source.
  *Stylized* mode bakes wide soft edge / cavity masks, painterly AO, a sky light with soft
  shadows and geometry-aware smoothing — maps meant to be layered for hand-painted looks
- Mask effect stacks: generators from baked maps, height gradients and seamless 3D noise
  (Perlin, Cellular, brush Strokes), filters **Warp**, **Slope Blur**, **Directional**, Blur, Levels
- **Stylize** filter layer: painterly simplification, value bands, warm lights / cool shadows,
  edge highlights, cavity darkening, simplified roughness, crisp metal, beveled height
- Smart layers: Edge highlight, Cavity dirt, Top light, Bottom shadow
- Declarative effect engine (JSON + WGSL per effect, UI generated from the definition):
  wear (metal / wood edge wear, paint peel, rust, scratches, stone chips, wood grain) and
  stylization (Stylization master filter, toon shading, posterize, palette lock, gradient map,
  edge highlight, cavity darken, hue shift by light), each with Light / Medium / Strong / Stylized presets
- Layer tree with folders, effect layers, anchors and per-channel blending
- **Native Wayland pen tablet** support (`zwp_tablet_v2`): pressure → size / flow, eraser end, barrel buttons
- `.s3dp` project files (zip: mesh + layer PNGs + baked maps + JSON)
- Export presets: Unreal (ORM / separate), Unity URP / HDRP, Godot, glTF, Stylized Unlit,
  Spec/Gloss, Sketchfab — channel packing, padding, DirectX / OpenGL normals, PNG / TGA / TIFF / EXR / JPG
- Russian and English UI

## Credits

Blend-mode formulas and the dark UI palette are adapted from
[PhotoCraft](https://github.com/storytold/photocraft) (MIT OR Apache-2.0); the tablet input
design follows its `photocraft-tablet` notes.
Icons: [Phosphor](https://phosphoricons.com) via `egui-phosphor` (MIT).

## Roadmap

- **M0** – window, model import, orbit camera, clay/UV/normal shading ✅
- **M1** – brush painting on the mesh (texture-space projection, seam dilation, undo, PNG export) ✅
- **M2** – layers, masks, Photoshop blend modes, material channels, eyedropper, `.s3dp` project files, Photoshop-style UI ✅
- **M3** – GPU mesh map baking from the mesh itself or a high-poly: AO, curvature, edges, cavity, thickness, sky light, normal, world normal, position; Realistic and Stylized modes ✅
- **M4** – mask generators (baked maps, height gradient, 3D noise), Warp / Slope Blur / Directional filters, Stylize filter layer, smart layers, Toon viewport ✅
- **M5** – effect engine, layer tree, wear / stylize effects, export presets ✅
- **M6** – native Wayland pen pressure ✅
- Next: X11 / Windows / macOS pen pressure, brush textures and tilt, release builds

Known issues: [KNOWN_ISSUES.md](KNOWN_ISSUES.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
