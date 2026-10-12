# Stylized3DPaint

Open-source 3D texture painter focused on stylized / hand-painted texturing.
Written in Rust with wgpu and egui.

**Status:** early development (milestone M0: model import + viewport).

## Build

```sh
cargo run --release -- path/to/model.fbx
```

Supported model formats: glTF/GLB, FBX, OBJ.

## Viewport controls

| Action | Input |
|--------|-------|
| Orbit  | Alt+LMB or MMB |
| Pan    | Alt+MMB, Shift+MMB or RMB |
| Zoom   | Wheel or Alt+RMB |
| Frame  | F |

## Roadmap

- **M0** – window, model import, orbit camera, clay/UV/normal shading ✅
- **M1** – brush painting on the mesh (texture-space projection, seam dilation, undo)
- **M2** – layers, masks, blend modes, channels, `.s3dp` project files
- **M3** – GPU bakers: AO, curvature, thickness, position, ID
- **M4** – stylized generators (edge highlight, cavity, height gradient, toon ramp, palettes), toon & PBR viewport
- **M5** – export presets for Unreal, Unity, Godot, Blender

## License

MIT OR Apache-2.0
