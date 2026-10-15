You are reviewing uncommitted work in this git worktree (see `git diff`): crates/interchange/src/scene/export.rs, crates/interchange/src/scene/export/tests.rs, crates/interchange/tests/sr_core_corpus.rs. Read-only: do not modify files, do not run cargo.

Intent: fix two sr-core scene export regressions. (1) Linked video+audio clips were exported both as the video's implicit audio and as an explicit audioMix placement, so re-import produced duplicate audio clips. Now matching linked one-to-one pairs use the video's own audio (preserving link, volume, mute, transition audio); independent/animated audio keeps an audioMix placement and reports lost linkage; picture-only assets declare hasAudio="false". (2) Default effects/params consumed the 100,000-item emitted-content export budget; now a separate checked 4,000,000-item traversal allowance applies.

Ground truth: crates/interchange/schema/sr-core/scene-render.xsd (videoType audio semantics, layerType/audioTrackType docs), /home/admin/src/scene-render/crates/sr-deliver/src/audio.rs (how the renderer actually plays video audio vs audioMix). Repo rules: AGENTS.md §0 never-crash (no unwrap/expect/panic/indexing on hostile data, checked math, bounded allocations).

Find real defects only, ranked: P1 = wrong audio on render (audio lost, doubled, wrong level/mute/timing, linkage claimed but not true, e.g. video muted but audio unmuted, speed/reverse/trimmed audio offset differs from video), a budget that can now be bypassed to cause unbounded work/memory, or a panic path; P2 = missing test coverage for a stated behavior, reports that misstate loss; P3 = clarity. For each: file:line, concrete scenario, minimal fix. End with a verdict line: SHIP, SHIP-WITH-FIXES, or BLOCK.

FOLLOW-UP PASS. Your previous review (below) was sent to the author, who reports fixing every item. Verify each fix is correct and complete, and look for regressions the fixes introduced. Report only remaining or new defects; do not re-report fixed issues or limitations the code already reports explicitly to the user (e.g. via Report warnings). Be calibrated: P1 only for concrete wrong output that is not reported, a panic, or unbounded work.

1. **P1 — Frame-aligned audio can still become silent at fractional frame rates.** [export.rs:1114](/home/admin/src/filmcraft-wt/fixexport/crates/interchange/src/scene/export.rs:1114)  
   At `24000/1001` fps, a matching linked pair spanning frames **9–10** passes `on_frame_grid`. However, `seconds()` exports its start as `0.375375`, while sr-deliver evaluates frame 9 at `0.37537499999999996`. The renderer’s strict start comparison excludes that frame; frame 10 equals the exclusive end. No source-time samples are recorded, so the entire audio placement is silent. Longer clips starting there lose their first frame of audio.

   **Minimal fix:** check embedding eligibility against the serialized endpoints and the renderer’s floating-point `k / fps` boundaries; use audioMix when those boundaries are unsafe. Add the one-frame `24000/1001`, frames 9–10 case as a regression.

Read-only review; no files modified and no cargo commands run.

BLOCK
