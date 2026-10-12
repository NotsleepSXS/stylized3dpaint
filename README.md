# Stylized3DPaint

Open-source 3D texture painter focused on stylized / hand-painted texturing.
Written in Rust with wgpu and egui.

**Status:** early development — milestone M2: layers, masks, material channels, Photoshop-style UI.

## Build

```sh
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
| Viewport mode | F1–F8 (Lit, Unlit, UV, Normals, Roughness, Metallic, Height, Emissive) |
| Undo / redo | Ctrl+Z / Ctrl+Shift+Z |
| New layer / duplicate / delete | Ctrl+Shift+N / Ctrl+J / Del |
| New / open / save / export | Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Shift+E |

## Features

- Texture-space painting with occlusion test and seam-free UV borders
- Layer stack shared by all texture sets: paint and fill layers, masks, 18 Photoshop blend modes, opacity
- Channels: Base Color, Roughness, Metallic, Height (shown as bump), Emissive
- Eyedropper picks the whole material under the cursor
- `.s3dp` project files (zip: mesh + layer PNGs + JSON)
- PNG export of all channel maps per texture set

## Credits

Blend-mode formulas and the dark UI palette are adapted from
[PhotoCraft](https://github.com/storytold/photocraft) (MIT OR Apache-2.0).
Icons: [Phosphor](https://phosphoricons.com) via `egui-phosphor` (MIT).

## Roadmap

- **M0** – window, model import, orbit camera, clay/UV/normal shading ✅
- **M1** – brush painting on the mesh (texture-space projection, seam dilation, undo, PNG export) ✅
- **M2** – layers, masks, Photoshop blend modes, material channels, eyedropper, `.s3dp` project files, Photoshop-style UI ✅
- **M3** – GPU bakers: AO, curvature, thickness, position, ID
- **M4** – stylized generators (edge highlight, cavity, height gradient, toon ramp, palettes), toon & PBR viewport
- **M5** – export presets for Unreal, Unity, Godot, Blender

## License

MIT OR Apache-2.0
