- **P1 — `blackHole` loses its sibling stacking value.** [resolve.rs:367](/home/admin/src/filmcraft-wt/fixdepth/crates/interchange/src/scene/resolve.rs:367) treats every double-valued `z` as stacking zero. However, the renderer exempts only `object3D`, `particles3D`, `ocean`, and `camera`; `blackHole` uses its numeric `z` converted to `i32` ([program.rs:1541](/home/admin/src/scene-render/crates/sr-eval/src/program.rs:1541)).

  Concrete scenario: sibling `<blackHole id="bh" mass="1" z="-7"/>` and `<shape id="s" shape="rect" width="8" height="8" z="-1"/>`. The renderer orders `bh, s`; this change orders `s, bh`. This also regresses previously accepted integer-spelled depths.

  **Minimal fix:** separate numeric validation from stacking semantics. Validate double-valued attributes as finite, but assign zero only to the renderer’s four exempt elements. For `blackHole`, match the renderer’s truncation toward zero and saturation to `i32`, retaining the original attribute text. Add a mixed-sibling regression and correct the test that currently asserts `blackHole` always stacks at zero.

No additional defects found in numeric rejection, integer bounds, or overlay text preservation. Review was static; no files modified and no cargo commands run.

BLOCK