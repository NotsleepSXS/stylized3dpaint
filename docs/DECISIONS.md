# Decisions

One line per decision: what was chosen and why.

- 2026-10-12 Blending and filters in linear space with 16-bit float layer buffers (spec §7), replacing the earlier Photoshop-like sRGB blending; colors are converted at input (picker/brush) and output (display/export).
- 2026-10-12 Effects are declared in JSON (`assets/effects/*.json`) + a WGSL body; a shared prelude provides mesh maps, 3D noise, warps and blending; UI is generated from the JSON (spec §2.5).
- 2026-10-12 Effects run per channel (color channels RGB, scalar channels in R, alpha = coverage); a generator in fill/content mode can output different values per channel (spec §4 multi-channel wear).
- 2026-10-12 Layer composition uses straight alpha with the W3C/PDF general blend formula (as Substance / PhotoCraft); the root accumulator starts opaque from the base material.
- 2026-10-12 Anchors capture an effect's output per texture set (content: all channels + alpha, mask: value); references pick a source (mask / height / luminance).
- 2026-10-12 Keep the Photoshop-style dock the user asked for, but order it as in Substance: Layers on top, Properties below (spec §8).
