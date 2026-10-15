- **P2 — Preserved transitions are reported as lost.** [import.rs:345](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import.rs:345)  
  Add an ordinary camera to a composition containing two layers and a crossfade between them. Camera promotion preserves the entire composition—including the transition—and suppresses both native layers. The subsequent `transition::apply` nevertheless reports “Unmapped transition: no native endpoint clip was retained.”  
  **Minimal fix:** skip native transition mapping and loss warnings for transitions fully preserved inside an overlay. Add a report assertion for this case.

No remaining P1 defects found in this pass. Review was read-only; no cargo commands were run.

SHIP