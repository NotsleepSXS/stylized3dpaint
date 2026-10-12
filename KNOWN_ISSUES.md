# Known issues

## Stylized bake: artifacts on every edge / polygon (reported 2026-10-12) — FIXED 2026-10-13

Fixed: edges / cavity / curvature now compare the interpolated normals of nearby surface points
with fixed probe directions and radii (bake.wgsl), so faceted smooth-shaded meshes read smooth
and the map is identical across UV seams. Remaining: faint thin rings in Cavity on very dense
concentric topology (monkey eyes) — low priority.

Original report:

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
