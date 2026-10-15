**S11 fix: two scene-export defects.**
1. `linked-audio` (18 cases), `crates/interchange/src/scene/export.rs:407`: export keeps a video's own audio as well as the explicit `audioMix` tracks, so re-importing gives one extra audio clip (2 → 3). Write each audio source exactly once, so that import → export → import keeps the clip count and the linkage. Decide which representation (video `audio` attributes vs an `audioMix` track) follows the XSD documentation and `/home/admin/src/scene-render/crates/sr-deliver` audio placement.
2. `10000-siblings` (the hostile case, test `scene_corpus_10000_siblings_export`), `export.rs:432`: default effects and parameters count against the export budget, so a document that imports within budget can't be exported. Separate the traversal budget from the emitted-content budget, or make the export budget for any project that import admits at least as large as the import budget, while keeping export work bounded. Export must still refuse truly oversized projects with an error, not a panic.

## Scope (you may edit)
- `crates/interchange/src/scene/export.rs`, plus helpers under `crates/interchange/src/scene/export/` if it is a module directory.
- The import side of linked audio, only if the cause turns out to be there. Say so explicitly in your reply.
- `crates/interchange/tests/sr_core_corpus.rs`: only the `linked-audio` and `10000-siblings` ignores and their known-regression entries.

## Shared rules for this fix
- `crates/interchange/tests/sr_core_corpus.rs` has your regressions as `#[ignore = "S11: …"]` tests plus entries in `KNOWN_REGRESSIONS`. Their failing cases, causes and proposed fixes are listed in `/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/agents/corpus/findings.json` (filter by `regression_fixture`). The fixtures under `crates/interchange/tests/fixtures/scene/corpus/` are your red tests.
- TDD: run the ignored test with `--ignored` on the VM and see it fail first. Then fix it, remove the `#[ignore]` and its `KNOWN_REGRESSIONS` entries, and see it pass. Add focused unit tests next to the code you change, with expected values worked out by hand from the XSD documentation and the reference renderer.
- In the test file, you may only remove your own ignores and known-regression entries. Do not touch other categories; another agent fixes them in parallel.
- Gate (VM): `cargo fmt --check && cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm`, then the corpus: `SR_CORE_REV=babeccf0ca0cb2603b8f6a1d3cb07388b7d7a2cc SR_CORE_DIR=$HOME/sr-core cargo test -p filmcraft-interchange --test sr_core_corpus -- --nocapture`. Your categories must report 0 failures. Report the other categories' counts unchanged or lower. A rise is a regression you caused.
- Reply HANDOFF with: the cause found, the fix, the red→green log, the corpus summary before and after, any borrowed scene-render code (file names), and the gate result.
