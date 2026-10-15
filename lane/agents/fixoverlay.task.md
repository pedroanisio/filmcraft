**S11 fix: overlay dependency closure on scene import.** Five regression categories, about 170 corpus cases, all in `crates/interchange/src/scene/import/overlay.rs`:
- `external-expression` (overlay.rs:49): sibling dependencies are refused. Build a bounded enclosing overlay instead, preserving the clocks.
- `generated-id` (overlay.rs:43): IDs created by generators are required to exist in the DOM. Keep the generators and defer the build-time references.
- `material-override` (overlay.rs:122): material IDs encoded in strings are missed. Include `materialOverride` dependencies.
- `voxel-palette`: palette material IDs are missed. Include their transitive dependencies.
- `geodesic-camera` (overlay.rs:152): the implicit black-hole dependency is lost. Keep the needed 3D context.

The dependency closure must stay bounded (node and depth budgets) and iterative. Derive dependency kinds from the XSD documentation and from the reference renderer (`/home/admin/src/scene-render/crates/sr-model`, `sr-eval`). Don't hard-code per-fixture special cases.

## Scope (you may edit)
- `crates/interchange/src/scene/import/overlay.rs`, plus a new sibling module under `scene/import/` if you need one.
- `crates/interchange/tests/sr_core_corpus.rs`: only the ignores and known-regression entries for these five categories.
- New fixtures under `crates/interchange/tests/fixtures/scene/overlay/`.

## Shared rules for this fix
- `crates/interchange/tests/sr_core_corpus.rs` has your regressions as `#[ignore = "S11: …"]` tests plus entries in `KNOWN_REGRESSIONS`. Their failing cases, causes and proposed fixes are listed in `/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/agents/corpus/findings.json` (filter by `regression_fixture`). The fixtures under `crates/interchange/tests/fixtures/scene/corpus/` are your red tests.
- TDD: run the ignored test with `--ignored` on the VM and see it fail first. Then fix it, remove the `#[ignore]` and its `KNOWN_REGRESSIONS` entries, and see it pass. Add focused unit tests next to the code you change, with expected values worked out by hand from the XSD documentation and the reference renderer.
- In the test file, you may only remove your own ignores and known-regression entries. Do not touch other categories; another agent fixes them in parallel.
- Gate (VM): `cargo fmt --check && cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm`, then the corpus: `SR_CORE_REV=babeccf0ca0cb2603b8f6a1d3cb07388b7d7a2cc SR_CORE_DIR=$HOME/sr-core cargo test -p filmcraft-interchange --test sr_core_corpus -- --nocapture`. Your categories must report 0 failures. Report the other categories' counts unchanged or lower. A rise is a regression you caused.
- Reply HANDOFF with: the cause found, the fix, the red→green log, the corpus summary before and after, any borrowed scene-render code (file names), and the gate result.
