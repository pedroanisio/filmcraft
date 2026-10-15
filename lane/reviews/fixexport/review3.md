1. **P1 — Frame-aligned audio can still become silent at fractional frame rates.** [export.rs:1114](/home/admin/src/filmcraft-wt/fixexport/crates/interchange/src/scene/export.rs:1114)  
   At `24000/1001` fps, a matching linked pair spanning frames **9–10** passes `on_frame_grid`. However, `seconds()` exports its start as `0.375375`, while sr-deliver evaluates frame 9 at `0.37537499999999996`. The renderer’s strict start comparison excludes that frame; frame 10 equals the exclusive end. No source-time samples are recorded, so the entire audio placement is silent. Longer clips starting there lose their first frame of audio.

   **Minimal fix:** check embedding eligibility against the serialized endpoints and the renderer’s floating-point `k / fps` boundaries; use audioMix when those boundaries are unsafe. Add the one-frame `24000/1001`, frames 9–10 case as a regression.

Read-only review; no files modified and no cargo commands run.

BLOCK