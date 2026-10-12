# Decisions

One line per decision: what was chosen and why.

- 2026-10-12 Blending and filters in linear space with 16-bit float layer buffers (spec §7), replacing the earlier Photoshop-like sRGB blending; colors are converted at input (picker/brush) and output (display/export).
- 2026-10-12 Effects are declared in JSON (`assets/effects/*.json`) + a WGSL body; a shared prelude provides mesh maps, 3D noise, warps and blending; UI is generated from the JSON (spec §2.5).
- 2026-10-12 Effects run per channel (color channels RGB, scalar channels in R, alpha = coverage); a generator in fill/content mode can output different values per channel (spec §4 multi-channel wear).
- 2026-10-12 Layer composition uses straight alpha with the W3C/PDF general blend formula (as Substance / PhotoCraft); the root accumulator starts opaque from the base material.
- 2026-10-12 Anchors capture an effect's output per texture set (content: all channels + alpha, mask: value); references pick a source (mask / height / luminance).
- 2026-10-12 Keep the Photoshop-style dock the user asked for, but order it as in Substance: Layers on top, Properties below (spec §8).
- 2026-10-12 Folders default to an isolated Normal blend; Passthrough is available and composites children straight into the stack below (effects then apply to that result).
- 2026-10-12 Masks are a stack of items (Fill, Paint, Effect) applied over black; new masks get a Fill (white/black) plus a Paint item.
- 2026-10-12 Generators output "white = occluded / thin / dirty" (AO, thickness, dirt) so they work directly as masks.
- 2026-10-12 Painted layers stored as raw RGBA f16 (premultiplied, linear) in project format 3; formats 1–2 are migrated on load.
- 2026-10-12 Expensive recomposites are throttled to ~10/s while editing instead of a 1/4-resolution preview (simpler, same responsiveness on target hardware).
- 2026-10-12 UI language: Russian by default, English via View menu; strings live in crates/s3dp-app/i18n/ru.json keyed by the English text.
