# Work report (spec "ТЗ: Stylize 3D Painter")

| Stage | Status | Notes |
|---|---|---|
| 1. Audit | done | docs/AUDIT.md, build + 18 tests green |
| 2. Effect core | done | declarative effects (JSON + WGSL, `crates/s3dp-core/effects`), shared prelude, generated properties UI, linear 16-bit float stack, Levels on layer content test |
| 3. Stack: folders & masks | done | node tree (paint / fill / folder / effect layer), effects on content and mask, per-channel blend, anchors, caches; tests 13.1 (folder blur, effect layer scope, anchor, roundtrip) |
| 4. Mask generators | done | 11 generators (curvature, AO, light, WSN, thickness, dirt, dust, leaks, UV border, mask editor, noise) + position gradient; seam / determinism / resolution tests |
| 5. Export | done | `crates/s3dp-export` (presets Unreal ORM/separate, Unity URP/HDRP, Godot, glTF, Stylized Unlit, Spec/Gloss, Sketchfab; channel packing, padding, DX/GL normals, PNG/TGA/TIFF/EXR/JPG) + export window |
