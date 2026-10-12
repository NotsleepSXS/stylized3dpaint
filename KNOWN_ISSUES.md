# Known issues

## Stylized bake: artifacts on every edge / polygon (reported 2026-10-12)

User report: in the Stylized bake every edge of the object is emphasized in Edges / Cavity
(and similar maps), with lots of artifacts.

Likely causes (to verify on the monkey test model):
- the edge/cavity probe casts "wall" rays along the tangent plane of the *interpolated*
  normal; on a faceted smooth-shaded mesh those rays graze neighbouring triangles, so every
  triangle edge reads as a tiny cavity/edge;
- the stylized `smoothstep(0, 0.6, x)` remap amplifies that noise;
- smoothing only blurs along one texel row/column per pass and does not cross UV seams.

Ideas: ignore hits closer than a fraction of the width or nearly parallel to the surface,
measure height against a fitted plane over several radii, threshold small responses,
multi-radius probing, compare with the triangle (face) normal instead of the interpolated one.
