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