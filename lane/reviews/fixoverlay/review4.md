Two remaining P1 defects:

1. **P1 — Ordinary audio after a 3D sequence child uses stale timing.** [import.rs:251](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import.rs:251)  
   In a 12-second project, use `<sequence><layer id="a" asset="v" threeD="true" clipOut="2"/><layer id="b" asset="v" clipOut="2"/></sequence>`, with audio-bearing video `v`. The original resolver gives `a` the inherited 12-second duration, placing `b` at 12 seconds. The audio re-resolution correctly places them at `[0,2)` and `[2,4)`, but this branch uses that result only for `a`: `b` already has a source map, so its soundtrack is silently omitted. The overlay renders both layers correctly.  
   **Minimal fix:** use the audio-resolved timeline consistently for audio-bearing layers, including their occupancy and animation timing. Add this sequence regression.

2. **P1 — Ordinary cameras remain disconnected from their rendered objects.** [overlay.rs:285](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import/overlay.rs:285)  
   With sibling `<camera id="view" z="-1000" fov="20"/>` and `<object3D id="box" primitive="box"/>`, neither references the other explicitly. Context promotion applies only to geodesic cameras, so preparation produces separate camera-only and object-only overlays. The object renders with the implicit camera, losing the specified projection; the camera-only overlay cannot restore it. The report claims preservation without reporting this loss. The XSD’s camera selection applies implicitly across the scene.  
   **Minimal fix:** promote ordinary cameras and their affected 3D content into a shared scene scope, preserving camera switching and coalescing covered clips. Add a camera-without-target regression.

The other reported fixes address their stated cases. Review was read-only; no cargo commands were run.

BLOCK