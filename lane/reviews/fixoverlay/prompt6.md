Review the uncommitted diff (git diff) in this worktree. It is on top of commit 12003949. It fixes this finding from a previous review:

- **P2 — Preserved transitions are reported as lost.** [import.rs:345](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import.rs:345)  
  Add an ordinary camera to a composition containing two layers and a crossfade between them. Camera promotion preserves the entire composition—including the transition—and suppresses both native layers. The subsequent `transition::apply` nevertheless reports “Unmapped transition: no native endpoint clip was retained.”  
  **Minimal fix:** skip native transition mapping and loss warnings for transitions fully preserved inside an overlay. Add a report assertion for this case.

No remaining P1 defects found in this pass. Review was read-only; no cargo commands were run.

SHIP

The intended rule: a transition is preserved only when one emitted overlay contains every endpoint and the transition's definition (an explicit transition node, or the enclosing sequence for an automatic transition). Partial preservation and separate overlays keep the native handling and warnings.

Verify the rule is implemented correctly. Check for any case where a transition is now silently dropped: skipped natively, but not actually rendered by an overlay. Check that the new code never panics, uses bounded work, and keeps budgets. Report only concrete defects. Be calibrated: P1 only for concrete wrong output that is not reported, a panic, or unbounded work. End with SHIP or BLOCK.
