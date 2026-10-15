You are reviewing uncommitted work in this git worktree (see `git diff`): crates/interchange/src/scene/export.rs, crates/interchange/src/scene/export/tests.rs, crates/interchange/tests/sr_core_corpus.rs. Read-only: do not modify files, do not run cargo.

Intent: fix two sr-core scene export regressions. (1) Linked video+audio clips were exported both as the video's implicit audio and as an explicit audioMix placement, so re-import produced duplicate audio clips. Now matching linked one-to-one pairs use the video's own audio (preserving link, volume, mute, transition audio); independent/animated audio keeps an audioMix placement and reports lost linkage; picture-only assets declare hasAudio="false". (2) Default effects/params consumed the 100,000-item emitted-content export budget; now a separate checked 4,000,000-item traversal allowance applies.

Ground truth: crates/interchange/schema/sr-core/scene-render.xsd (videoType audio semantics, layerType/audioTrackType docs), /home/admin/src/scene-render/crates/sr-deliver/src/audio.rs (how the renderer actually plays video audio vs audioMix). Repo rules: AGENTS.md §0 never-crash (no unwrap/expect/panic/indexing on hostile data, checked math, bounded allocations).

Find real defects only, ranked: P1 = wrong audio on render (audio lost, doubled, wrong level/mute/timing, linkage claimed but not true, e.g. video muted but audio unmuted, speed/reverse/trimmed audio offset differs from video), a budget that can now be bypassed to cause unbounded work/memory, or a panic path; P2 = missing test coverage for a stated behavior, reports that misstate loss; P3 = clarity. For each: file:line, concrete scenario, minimal fix. End with a verdict line: SHIP, SHIP-WITH-FIXES, or BLOCK.

SECOND PASS. Your first review (below) was sent to the author, who reports fixing every item. Verify each fix is correct and complete, and look for regressions the fixes introduced. Report only remaining or new defects.

1. **P1 — Video transitions silently change the audio mix.** [export.rs:1128](/home/admin/src/filmcraft-wt/fixexport/crates/interchange/src/scene/export.rs:1128)  
   Two matching linked AV pairs with a video dissolve but no audio transition now become embedded audio. Exported transitions omit `audio`, whose schema default is `crossfade`; the renderer applies those gains and plays transition handles. A native hard audio cut therefore becomes overlapping, attenuated audio.  
   **Minimal fix:** gate embedding on compatible transition semantics; otherwise retain audioMix placements and report lost linkage. Add a video-only dissolve regression that checks audio behavior.

2. **P1 — Duplicate clip IDs can associate and suppress the wrong audio.** [export.rs:1128](/home/admin/src/filmcraft-wt/fixexport/crates/interchange/src/scene/export.rs:1128)  
   In a malformed project, put two video clips on different tracks with the same `ClipId`, distinct sources and distinct one-to-one link groups. `layer_audio` overwrites one pairing because it keys only by `ClipId`. Both pictures then use the surviving pairing’s volume/mute and mark its audio as embedded. The other audio remains in audioMix, doubling that source. Existing duplicate detection is local to each video track.  
   **Minimal fix:** reject duplicate clip IDs across the sequence before pairing, or key pairing and suppression by unambiguous track/item locations.

3. **P2 — Animated linked-audio fallback lacks coverage.** [export/tests.rs:745](/home/admin/src/filmcraft-wt/fixexport/crates/interchange/src/scene/export/tests.rs:745)  
   This test changes timing, gain and pan together; the existing animation test uses unlinked audio. Neither exercises an otherwise matching linked pair rejected solely because its volume is animated. Removing the animation guard could silently flatten volume while all these tests still pass.  
   **Minimal fix:** add a matching linked pair with volume keys; assert one audioMix placement, picture `hasAudio="false"`, preserved gain keys and a linkage-loss warning.

No new budget bypass or panic path identified. Review was read-only; cargo was not run.

BLOCK
