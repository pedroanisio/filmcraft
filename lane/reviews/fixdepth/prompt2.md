You are reviewing uncommitted work in this git worktree (see `git diff`): crates/interchange/src/scene/resolve.rs and crates/interchange/tests/sr_core_corpus.rs. Read-only: do not modify files, do not run cargo.

Intent: sr-core scene import parsed every `z` attribute as an integer sibling-stacking index, so 3D nodes with geometric double z (camera, object3D, particles3D, ocean, blackHole) failed with '"-0.5" is not an integer'. Now the XSD type table decides: geometric xs:double z must be finite and contributes zero to stacking; integer stacking keeps its bounds; original text is preserved for overlay serialization.

Ground truth: crates/interchange/schema/sr-core/scene-render.xsd (nodeCoreAttributes/z and the element types that override z/depth), /home/admin/src/scene-render/crates/sr-model (parse.rs, build.rs) and sr-eval/src/program.rs for how the renderer orders siblings. Repo rules: AGENTS.md §0 never-crash.

Find real defects only, ranked: P1 = wrong paint order versus the renderer (e.g. 3D nodes that do participate in 2D stacking, shapes with threeD, elements whose z type the table lookup misses, inherited/default z), hostile input that now panics or bypasses bounds (NaN, inf, huge exponents, whitespace, suffixes), lost or altered z text on export; P2 = missing test coverage; P3 = clarity. Note: two harness tests index KNOWN_REGRESSIONS[1] positionally; that is known and being fixed separately, do not report it. For each: file:line, concrete scenario, minimal fix. End with a verdict line: SHIP, SHIP-WITH-FIXES, or BLOCK.

SECOND PASS. Your first review (below) was sent to the author, who reports fixing every item. Verify each fix is correct and complete, and look for regressions the fixes introduced. Report only remaining or new defects.

- **P1 — `blackHole` loses its sibling stacking value.** [resolve.rs:367](/home/admin/src/filmcraft-wt/fixdepth/crates/interchange/src/scene/resolve.rs:367) treats every double-valued `z` as stacking zero. However, the renderer exempts only `object3D`, `particles3D`, `ocean`, and `camera`; `blackHole` uses its numeric `z` converted to `i32` ([program.rs:1541](/home/admin/src/scene-render/crates/sr-eval/src/program.rs:1541)).

  Concrete scenario: sibling `<blackHole id="bh" mass="1" z="-7"/>` and `<shape id="s" shape="rect" width="8" height="8" z="-1"/>`. The renderer orders `bh, s`; this change orders `s, bh`. This also regresses previously accepted integer-spelled depths.

  **Minimal fix:** separate numeric validation from stacking semantics. Validate double-valued attributes as finite, but assign zero only to the renderer’s four exempt elements. For `blackHole`, match the renderer’s truncation toward zero and saturation to `i32`, retaining the original attribute text. Add a mixed-sibling regression and correct the test that currently asserts `blackHole` always stacks at zero.

No additional defects found in numeric rejection, integer bounds, or overlay text preservation. Review was static; no files modified and no cargo commands run.

BLOCK
