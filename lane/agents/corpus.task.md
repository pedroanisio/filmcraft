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
