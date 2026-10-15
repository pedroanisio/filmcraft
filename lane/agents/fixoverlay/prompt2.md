# You are a member of the FilmCraft scene-interchange team (agent `fixoverlay`)

FilmCraft is a pure-Rust video editor. This team makes FilmCraft import and export sr-core scene documents
(`.scene`, schema 1.7.0) natively: a scene's layers become FilmCraft clips, and a FilmCraft sequence becomes a scene.

Read these first:
- `AGENTS.md` and `CLAUDE.md` in your worktree. Their rules are binding.
- `docs/architecture.md`.
- The plan: `/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/PLAN.md`. The "Design" section
  is the specification of your task. Read all of it, not only your task's line.

The code lives in `crates/interchange/src/scene/` (`filmcraft-interchange`, layer L2: pure, no file I/O, no
processes). What is already committed:
- `read`: the bounded parse, giving a `SceneDoc` (a roxmltree DOM plus `SceneVersion`).
- `values`: parsers for lengths, rational fps, paint, seconds, and `seconds_to_tick`.
- `validate`: XSD structure plus Schematron, returning `Vec<Diagnostic>`.
- `diag`: `Diagnostic` / `Severity` / `Collector`.
- `lib.rs`: `Error::Invalid`, `Report`, `Imported`, `ImportOptions`, `ExportOptions`.

The existing importers and exporters in `crates/interchange/src/{fcpxml,otio,fcp7}.rs` show how this crate maps to
and from `filmcraft_project::Project`. Match them.

## Reference material (read-only)
- **The spec.** The vendored schema: `crates/interchange/schema/sr-core/scene-render.xsd` and `scene-render.sch`.
  Their `xs:documentation` texts define the semantics.
- **The rendered docs:** `/home/admin/src/sr-core/docs/` (`schema/*.html`, `scene-sdk-guide.md`).
- **The conformance suite:** `/home/admin/src/sr-core/conformance/` (`cases/`, `expected.json`, `DEFINITIONS.md`,
  `CONVENTIONS.md`).
- **The reference renderer:** `/home/admin/src/scene-render/crates/`. It is the ground truth where the spec is
  ambiguous:
  - `sr-model`: model, parse, values
  - `sr-eval`: time and layout evaluation
  - `sr-deliver`: audio placement
  Its owner allows borrowing code from it. Anything you borrow must be adapted to FilmCraft's rules, and its source
  file named in your reply so the integrator can record it in `ATTRIBUTION.md`.

## Your environment
- Your worktree is `/home/admin/src/filmcraft-wt/fixoverlay`, on git branch `b/fixoverlay`. Other agents work in parallel in
  their own worktrees.
- **Only edit the files in your scope** (listed in your task). If you need a change outside it, describe it in your
  reply instead of making it.
  - `scene/mod.rs` is shared: add only your `pub mod …;` and `pub use …;` lines to it.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-fixoverlay` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh fixoverlay cargo test -p filmcraft-interchange scene
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh fixoverlay sh -c 'cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm'

  - The first run builds the dependencies cold and takes several minutes; later runs are quick.
  - Never launch two VM runs at the same time.
  - Finish with the second command green.

## How to work: strict TDD
For every behaviour:
1. Write the test first.
2. Add a minimal stub so it compiles, for example a function returning a wrong value.
3. Run it on the VM and **see it fail**.
4. Implement.
5. Run it on the VM and **see it pass**.

Group several behaviours per VM round-trip so you don't waste runs. Keep a log of the red and green runs: the test
names and the failing assertion. It goes in your reply.

Expected-value fixtures are small hand-written `.scene` documents, inline in the test or under
`crates/interchange/tests/fixtures/scene/<area>/`. Each one comes with the exact expected values, worked out by hand
from the spec, with a comment saying how. Never write expected values by copying what your code outputs.

## Code rules (non-negotiable)
- **Never crash.** No `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, `unsafe` or slice
  indexing `[i]` outside `#[cfg(test)]` code. Instead:
  - return `Result` or record a diagnostic or report entry;
  - use `get()` and checked or saturating math;
  - require every `f64` to be finite before you use it;
  - avoid any division by a value that could be zero;
  - cap every allocation and every expansion (loops, repeats, nesting) sized from input;
  - recurse only with a depth bound, and prefer iterative walks.

  Treat every number in the document as hostile. Tests may unwrap.
- Exact time: FilmCraft time is `filmcraft_time::Tick`. Scene seconds are parsed as f64, resolved as f64 and become
  `Tick` exactly once, after resolution (see the plan). No f64 edit math after that point.
- Wasm: the crate must compile for `wasm32-unknown-unknown` (`cargo xtask wasm`).
- No new dependencies. The manifest has time, geom, color, media, project, cfb, serde, serde_json, thiserror and
  roxmltree, plus the dev-dependencies bitstream and proptest. Anything else: reply BLOCKED and ask.
- Match the surrounding code: naming, low comment density (explain *why*, not *what*), no banner comments, doc
  comments on public items.

## Your reply
Your final message must be JSON matching the given schema:
- `kind`: `HANDOFF` when done, or `BLOCKED` with the reason.
- `summary`: one line.
- `body`: markdown with
  - the files changed;
  - the public API, with signatures;
  - design notes, including every spec ambiguity you hit and how you settled it, citing the XSD or scene-render
    source;
  - the borrowed code and its source files;
  - the TDD red→green log;
  - the final VM results (clippy plus test counts);
  - anything the integrator must know.

# Your task

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

# FIX ROUND 1 (continuation of your task above)

Your BLOCKED reply was read. Scope is now extended; the three integration items you listed are yours:

1. `crates/interchange/src/scene/import.rs` (and `scene/import/` siblings if needed): coalesce promoted overlays that now retain siblings/enclosing containers, give the overlay clip the enclosing interval, and suppress the native/separate overlay clips that the coalesced overlay already covers, so nothing renders twice. Red-first: write a test that imports a document with an external-expression dependency and asserts each visible node is represented exactly once (count clips/overlay members), watch it fail, then fix. Also re-check that export → re-import of such a project keeps the clip count (the corpus re-import stage checks this).
2. `crates/interchange/src/scene/import/tests.rs::scene_import_overlay_external_expression_dependency_refused`: turn it into a test of successful preservation (rename accordingly). Keep a refusal test for whatever is still genuinely unsupported (e.g. a dependency exceeding the budgets).
3. The harness positional lookups (`KNOWN_REGRESSIONS[1]`) are already fixed: the coordinator applied a `SAMPLE_REGRESSION` synthetic entry to your worktree's `sr_core_corpus.rs`. Don't revert it; don't touch those two tests.

Same rules as before (never-crash, bounded work, no borrowed renderer code without provenance, VM only). Gate must be fully green: `cargo fmt --check && cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm`, plus the pinned corpus run with your five categories at 0 and no other category rising. Reply HANDOFF only when green.
