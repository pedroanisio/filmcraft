You are reviewing uncommitted work in this git worktree (see `git diff`): crates/interchange/src/scene/export.rs, crates/interchange/src/scene/export/tests.rs, crates/interchange/tests/sr_core_corpus.rs. Read-only: do not modify files, do not run cargo.

Intent: fix two sr-core scene export regressions. (1) Linked video+audio clips were exported both as the video's implicit audio and as an explicit audioMix placement, so re-import produced duplicate audio clips. Now matching linked one-to-one pairs use the video's own audio (preserving link, volume, mute, transition audio); independent/animated audio keeps an audioMix placement and reports lost linkage; picture-only assets declare hasAudio="false". (2) Default effects/params consumed the 100,000-item emitted-content export budget; now a separate checked 4,000,000-item traversal allowance applies.

Ground truth: crates/interchange/schema/sr-core/scene-render.xsd (videoType audio semantics, layerType/audioTrackType docs), /home/admin/src/scene-render/crates/sr-deliver/src/audio.rs (how the renderer actually plays video audio vs audioMix). Repo rules: AGENTS.md §0 never-crash (no unwrap/expect/panic/indexing on hostile data, checked math, bounded allocations).

Find real defects only, ranked: P1 = wrong audio on render (audio lost, doubled, wrong level/mute/timing, linkage claimed but not true, e.g. video muted but audio unmuted, speed/reverse/trimmed audio offset differs from video), a budget that can now be bypassed to cause unbounded work/memory, or a panic path; P2 = missing test coverage for a stated behavior, reports that misstate loss; P3 = clarity. For each: file:line, concrete scenario, minimal fix. End with a verdict line: SHIP, SHIP-WITH-FIXES, or BLOCK.

FOLLOW-UP PASS. Your previous review (below) was sent to the author, who reports fixing every item. Verify each fix is correct and complete, and look for regressions the fixes introduced. Report only remaining or new defects; do not re-report issues already fixed. Be calibrated: P1 only for concrete wrong output, panic, or unbounded work.

1. **P1 — Embedded audio loses subframe timing.** [export.rs:1103](/home/admin/src/filmcraft-wt/fixexport/crates/interchange/src/scene/export.rs:1103)  
   Matching linked clips starting at 0.01 seconds in a 24-fps sequence qualify for embedding. However, sr-deliver samples layer clocks only at frame boundaries, and `Placement::Mapped` skips samples without a current frame’s source time. Audio therefore starts at 0.041667 seconds, losing approximately 31.7 ms. A placement entirely between frame boundaries becomes silent.  
   **Minimal fix:** retain audioMix placements and report lost linkage when either endpoint is off the render frame grid. Add a matching linked pair with subframe boundaries as a regression.

2. **P1 — Reversed or speed-adjusted embedded audio plays the wrong final-frame source span.** [export.rs:1106](/home/admin/src/filmcraft-wt/fixexport/crates/interchange/src/scene/export.rs:1106)  
   Equal speed/reverse fields permit embedding, but the renderer’s [mapped-clock fallback](/home/admin/src/scene-render/crates/sr-audio/src/mix.rs:477) advances at **forward 1×** when the next frame is outside the layer. For a one-second reversed clip using source `[5,6)` at 24 fps, the final frame advances from approximately 5.041667 toward 5.083333 instead of descending toward 5.0. A 2× clip similarly slows to 1× for its final frame.  
   **Minimal fix:** restrict embedding to forward 1× playback until the renderer handles boundary extrapolation correctly; retain audioMix and report lost linkage otherwise. For speed-adjusted fallback, emit `preservePitch="false"` to preserve FilmCraft’s varispeed behavior. Add reverse and speed boundary regressions.

Review was read-only; cargo was not run.

BLOCK
