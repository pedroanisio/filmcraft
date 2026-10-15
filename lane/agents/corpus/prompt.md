# You are a member of the FilmCraft scene-interchange team (agent `corpus`)

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
- Your worktree is `/home/admin/src/filmcraft-wt/corpus`, on git branch `b/corpus`. Other agents work in parallel in
  their own worktrees.
- **Only edit the files in your scope** (listed in your task). If you need a change outside it, describe it in your
  reply instead of making it.
  - `scene/mod.rs` is shared: add only your `pub mod …;` and `pub use …;` lines to it.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-corpus` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh corpus cargo test -p filmcraft-interchange scene
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh corpus sh -c 'cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm'

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

**S11 (part 1): corpus and oracle tests for scene import and export.**

Scope: tests only. Product code changes are **out of scope**. When a corpus case exposes a bug (a panic, an import
that is wrong, an export that sr-core rejects), do two things:
- Add the case as a required in-CI regression fixture, marked `#[ignore = "S11: <one-line cause>"]`.
- Describe it in your reply: the file, the line, the cause and a proposed fix.

The integrator dispatches the fixes. vpkg oracles are not part of this task; they come once `filmcraft-vpkg` (S10) lands.

## Scope (you may create or edit)
- `crates/interchange/tests/sr_core_corpus.rs` (new). The lane runs it as
  `SR_CORE_DIR=$HOME/sr-core cargo test -p filmcraft-interchange --test sr_core_corpus -- --nocapture`.
- `crates/interchange/tests/fixtures/scene/corpus/` (new). Small hostile and regression `.scene` fixtures, each under
  8 KiB. They are original work, so follow the repo's convention for test fixtures; check whether text fixtures
  under `tests/fixtures/scene/` already carry sidecars, and do the same.
- `crates/interchange/tests/support/` only if you need a shared helper.
- The existing `crates/interchange/tests/scene_conformance.rs` (validation parity) stays as it is.

## What the corpus test checks
Read the pinned sr-core revision's file list at runtime; don't hard-code a count. Pinned:
`babeccf0ca0cb2603b8f6a1d3cb07388b7d7a2cc`. Print its `git rev-parse HEAD`, or the `SR_CORE_REV` env var, when
available. The inputs are every `conformance/cases/*.xml`, every scene in `conformance/srep_cases/`, and the scene
examples in `docs/` (if any). For each one:
1. Run `filmcraft_interchange::scene::import` (the same entry point the engine uses, with `ImportOptions` for a strict
   import, then a lenient one) inside `std::panic::catch_unwind`. **No case panics.** A panic is a failure that
   names the case.
2. Check the result:
   - A case `expected.json` marks valid, or one with no error findings, must import strictly.
   - A case that is invalid must return `Error::Invalid` with at least one diagnostic whose code matches the
     expectation (reuse the NOT_PORTED list logic from `scene_conformance.rs`).
3. For every strict import that succeeds, export the first sequence with `scene::export`.
   - Re-import it (it must succeed, and the clip count is preserved within the reported losses).
   - **Oracle:** when `python3 -c 'import lxml'` works, run
     `python3 -m sr_core.scene check --schema <file>` with `PYTHONPATH=$SR_CORE_DIR` and `cwd` set to a temp dir under
     `CARGO_TARGET_TMPDIR`. Every export must pass. When lxml is missing, print that the schema oracle was skipped.
4. Print a summary: the number of cases, imported, refused as expected, exported, oracle-checked, and the skipped
   cases with the reason. Fail the test listing **every** failing case, not only the first.

Without `SR_CORE_DIR` the corpus test prints "skipped" and passes. **Required in CI regardless**: a second test runs
the same pipeline (no panic, strict/lenient, export, re-import) over `tests/fixtures/scene/**/*.scene` checked into
the repo, plus a set of hostile documents you add:
- deep nesting at the read limit, plus one deeper
- 10 000 sibling layers within the node budget
- `loop` = u32::MAX
- `speed` = 1e-300
- NaN-like and inf strings in number attributes
- timeRemap with zero keys
- groups referencing each other through matte
- a 2^53+1-tick duration
- rational fps `30000/0`

Each must be refused or imported without panic, and in bounded time (put a coarse wall-clock bound in the
assertion message only, not a timing assertion).

## Running the oracle on the VM
`~/sr-core` may not exist on the VM yet. Check first with `vm-agent.sh corpus ls ~/sr-core`. If it is missing, report
that in your reply rather than copying it; the integrator ships it. Run the corpus with:

    vm-agent.sh corpus sh -c 'SR_CORE_DIR=$HOME/sr-core cargo test -p filmcraft-interchange --test sr_core_corpus -- --nocapture'

## Gate (VM only, through `vm-agent.sh corpus …`)
```
cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask assets
```
Run the corpus with `SR_CORE_DIR` once `~/sr-core` is on the VM. Don't commit. Reply HANDOFF with:
- the summary numbers
- every failing or ignored case, with its cause and proposed fix
- the gate result

# FIX ROUND 1 (continuation of your task above)
Your work is still uncommitted in this worktree. An independent review found that the harness can pass while hiding failures. Fix each defect test-first where you can: write a self-test of the harness logic that fails, then the fix. Product code is still out of scope.

1. `sr_core_corpus.rs:257`: `skip_reason` drops the whole expectation. Skip only the diagnostic-code assertions that are unsupported or NOT_PORTED, and keep the valid/invalid expectation (for example, `srep-0073-default-no-finding` expects `valid: true`, so a strict-import error there must be a failure).
2. `sr_core_corpus.rs:249`: projects from lenient imports are discarded. Run the export, re-import and oracle round trip on **every** successful import mode (strict and lenient), and label each failure with its mode.
3. `sr_core_corpus.rs:423`: the ignored `10000-siblings` regression can pass without exporting anything. Assert that import succeeds with 10 000 clips (or the exact count the generator yields), and that export and re-import complete.
4. `sr_core_corpus.rs:528`: the known-regression match is a substring of a combined error string, so a new diagnostic in the same error is hidden. Match each known regression by stage (import, export or re-import) and by its exact normalized set of diagnostic codes. Any extra diagnostic, or a different stage, is a failure.
5. `sr_core_corpus.rs:234`: the Python calls have no deadline. Spawn with a deadline: poll `try_wait` with a 120 s cap per call, then kill and reap. Add a timeout as an aggregated failure. Std only.

Then re-run on the VM:
- the gate: `cargo fmt --check -p filmcraft-interchange && cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask assets`
- the corpus: `SR_CORE_REV=babeccf0ca0cb2603b8f6a1d3cb07388b7d7a2cc SR_CORE_DIR=$HOME/sr-core cargo test -p filmcraft-interchange --test sr_core_corpus -- --nocapture`

Update findings.json. Lenient-mode round trips may expose new product bugs; record those as new ignored regressions with fixtures, as before. Reply HANDOFF with the new summary numbers, any new failure categories (each with file:line, cause and proposed fix), and the gate result.
