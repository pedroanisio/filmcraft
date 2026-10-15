1. **P1 — Embedded audio loses subframe timing.** [export.rs:1103](/home/admin/src/filmcraft-wt/fixexport/crates/interchange/src/scene/export.rs:1103)  
   Matching linked clips starting at 0.01 seconds in a 24-fps sequence qualify for embedding. However, sr-deliver samples layer clocks only at frame boundaries, and `Placement::Mapped` skips samples without a current frame’s source time. Audio therefore starts at 0.041667 seconds, losing approximately 31.7 ms. A placement entirely between frame boundaries becomes silent.  
   **Minimal fix:** retain audioMix placements and report lost linkage when either endpoint is off the render frame grid. Add a matching linked pair with subframe boundaries as a regression.

2. **P1 — Reversed or speed-adjusted embedded audio plays the wrong final-frame source span.** [export.rs:1106](/home/admin/src/filmcraft-wt/fixexport/crates/interchange/src/scene/export.rs:1106)  
   Equal speed/reverse fields permit embedding, but the renderer’s [mapped-clock fallback](/home/admin/src/scene-render/crates/sr-audio/src/mix.rs:477) advances at **forward 1×** when the next frame is outside the layer. For a one-second reversed clip using source `[5,6)` at 24 fps, the final frame advances from approximately 5.041667 toward 5.083333 instead of descending toward 5.0. A 2× clip similarly slows to 1× for its final frame.  
   **Minimal fix:** restrict embedding to forward 1× playback until the renderer handles boundary extrapolation correctly; retain audioMix and report lost linkage otherwise. For speed-adjusted fallback, emit `preservePitch="false"` to preserve FilmCraft’s varispeed behavior. Add reverse and speed boundary regressions.

Review was read-only; cargo was not run.

BLOCK