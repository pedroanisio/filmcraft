# You are a member of the FilmCraft scene-interchange team (agent `vpkg`)

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
- Your worktree is `/home/admin/src/filmcraft-wt/vpkg`, on git branch `b/vpkg`. Other agents work in parallel in
  their own worktrees.
- **Only edit the files in your scope** (listed in your task). If you need a change outside it, describe it in your
  reply instead of making it.
  - `scene/mod.rs` is shared: add only your `pub mod …;` and `pub use …;` lines to it.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-vpkg` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh vpkg cargo test -p filmcraft-interchange scene
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh vpkg sh -c 'cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm'

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

**S10 (round 1): new crate `filmcraft-vpkg`, which reads, verifies and writes scene video packages (`.vpkg.zip`)
in format 1.0 and 1.1 (SREP 1, SREP 6). It is pure and in-memory. A later round wires it into the engine.**

This task is not in `crates/interchange/src/scene/`. Ignore the scene-specific scope in the environment section above;
this section gives your scope.

## Spec (read all of it)
- `/home/admin/src/sr-core/srep/srep-0001.md`: format 1.0, the zip layout, `vpkg.json`, and what `verify` checks.
- `/home/admin/src/sr-core/srep/srep-0006.md`: format 1.1, `packed.metaSha256`, and the safe-reading rules.
- `/home/admin/src/sr-core/srep/srep-0004.md`: versioning. A 1.N reader reads a later 1.M with warnings and
  refuses other majors.
- `/home/admin/src/sr-core/docs/vpkg-howto.md`.
- The reference implementation, `scenerender-vpkg` in `/home/admin/src/sr-core/sr_core/vpkg/`, is MIT. Read
  `archive.py`, `manifest.py`, `pack.py`, `readme.py` and `refs.py`, and treat it as ground truth where the SREPs
  are ambiguous. You may adapt it, but name every file you borrow from.
- The manifest schema is `/home/admin/src/sr-core/sr_core/vpkg/schema/vpkg-1.schema.json`.

## What exists that you build on
- `filmcraft_bitstream::flate`:
  - `inflate_with_consumed(src, limit) -> Result<(Vec<u8>, usize), FlateError>`
  - `inflate_exact`
  - `deflate`
  - `adler32`
- `filmcraft_bitstream::crc32` and `filmcraft_bitstream::sha256`. Read their APIs in `crates/bitstream/src/`.
- `filmcraft_interchange::scene::read` gives you the bounded scene parse. Use it to scan a packaged scene's references.

## Scope (you may create or edit)
- `crates/vpkg/` (new). `Cargo.toml` should copy the layout of a sibling crate such as `crates/interchange/Cargo.toml`:
  - `version.workspace`, `[lints] workspace = true`
  - dependencies limited to workspace entries already present: `filmcraft-bitstream`, `filmcraft-interchange`,
    `serde`, `serde_json`, `thiserror`, `roxmltree`
  - dev-dependencies `proptest`
  - **no new external crates**
- The root `Cargo.toml` `[workspace.dependencies]`: add one line, `filmcraft-vpkg = { path = "crates/vpkg" }`.
- `xtask/src/main.rs`: add `("vpkg", 3)` to the layer table, and nothing else.
- `crates/vpkg/schema/vpkg-1.schema.json`: copy it unchanged from sr-core. Add a `vpkg-1.schema.json.attribution`
  sidecar copied from the style of `crates/interchange/schema/sr-core/*.attribution`. `cargo xtask assets` must pass.
- `ATTRIBUTION.md`: rows for the vendored schema and for any adapted sr-core code, in the style of the existing
  sr-core rows.
- `Cargo.lock`: let **cargo** update it on the VM, then copy it back with
  `vm-agent.sh vpkg sh -c 'cargo metadata --format-version 1 >/dev/null && cat Cargo.lock' > Cargo.lock.new`.
  Check that the file is a clean lockfile, with no ssh noise, and that the diff adds only `filmcraft-vpkg`. Then move it
  into place. Never hand-edit it.

## Design
Four modules, all pure (`std` only: no I/O, no processes, no clock).

### `zip`
A bounded ZIP reader and writer.

Reader:
- Locate the EOCD by a bounded backwards scan, at most 65 557 bytes.
- Use the central directory as the source of truth and cross-check each local header: name, method, sizes, CRC
  flags.
- Methods are 0 (stored) and 8 (deflate). Bit 3 (data descriptor) is handled.
- Check CRC-32, and check that the compressed size is consumed exactly (`inflate_with_consumed`).
- Reject:
  - ZIP64. Document this as a limit and test it.
  - encrypted entries
  - multi-disk archives
  - overlapping entries
  - a comment that is truncated or past the end
  - a duplicate name, including names that differ only by case, since case-insensitive filesystems collide
  - unsafe names: absolute, `..`, backslash, a drive letter, NUL or control characters, empty segments
  - symlinks, via the Unix mode in the external attributes
- Budgets come from a `Limits` struct: entry count, total uncompressed bytes, per-entry bytes, name length, and the
  overall memory budget. They are checked against **actual** inflated output, never only the advertised sizes.

Writer:
- Output is reproducible: a fixed timestamp (1980-01-01, or a caller-given `SOURCE_DATE_EPOCH`) and modes 0644, or
  0755 for scripts.
- Media is stored and everything else is deflated, as in `pack.py`.
- Entries are written in the given order.
- Output must be readable by Python's `zipfile`. You can check this locally: `python3 -c 'import zipfile,sys;
  zipfile.ZipFile(sys.argv[1]).testzip()'` is allowed. Python is not cargo.

### `manifest`
- Use typed serde structs for the parts FilmCraft reads, and keep the full JSON `Value` so unknown keys survive.
- Validation follows the schema and SREP 1's semantic rules:
  - `format`
  - the `formatVersion` major/minor policy
  - exactly one `primary` scene
  - unique step ids
  - known `needs`
  - no pipeline cycles; check iteratively, with a bound
  - `files[]` paths unique and safe
  - sizes and sha256 well-formed
- Hand-code the JSON Schema checks that matter, because there is no jsonschema crate. List which schema constraints
  you enforce and which you don't in the README of the crate. Unknown keys in a newer 1.M give warnings.

### `verify`
`read(bytes, &Limits) -> Result<Package, Error>`. It does the **complete non-writing validation** of SREP 6:
- `vpkg.json` is the first entry, followed by `vpkg.schema.json` and `README.md`.
- Every `files[]` entry exists under `project/` with the listed size and SHA-256.
- No stray entries.
- In 1.1, `packed.metaSha256` covers README.md and vpkg.schema.json. In 1.0, a missing `metaSha256` is a warning.
- Reference closure: every packaged primary and variant scene is parsed with `scene::read`, and each input
  reference resolves to one of:
  - a packaged file
  - a `fetch` entry
  - a pipeline output glob

  Classify input versus output attributes exactly as `refs.py` does. Drafts are exempt.
- Findings are `Diagnostic`-like structs (severity, code, path, message); reuse the shape of
  `filmcraft_interchange::scene::diag` if that fits.

`Package` gives:
- the manifest
- the primary scene path
- a list of verified `(path, bytes)` entries, held only once
- warnings

Nothing is returned if any error was found.

### `write`
`pack(input: &PackInput) -> Result<Vec<u8>, Error>` builds a **format 1.1** package from:
- a primary scene (path plus bytes)
- the files the scene reads (path plus bytes plus role)
- package metadata (id slug, semver version, title, created)

It works like this:
- It computes `files[]`, `video` from the primary scene (width, height, fps, duration, audio, captions) and `packed`
  (tool `filmcraft`, fileCount, totalSize, metaSha256).
- It generates `README.md`, porting `readme.py`. Keep it minimal but deterministic.
- It writes the zip in SREP 1 order.
- It refuses inputs that are absent, unsafe or duplicated, and remote references.

FilmCraft does not resolve fonts, run lint, or build pipelines. Document what is out of scope, with the reason.

## Requirements
- Follow the AGENTS.md "never crash" rules throughout. Every hostile zip or manifest returns `Err` or a diagnostic.
- Write a property test: `read(pack(x))` round-trips the files and their bytes over generated small packages.
- **Oracle tests:**
  - Take 2–3 real packages produced by `scenerender-vpkg pack` and commit them as tiny test fixtures under
    `crates/vpkg/tests/fixtures/`.
  - To build them, run `python3 -m sr_core.vpkg pack` locally from `/home/admin/src/sr-core` on a tiny project you
    create in the scratch area of your worktree, under `target/`. Then copy the result in.
  - Each fixture gets a `.attribution` sidecar: original work generated by this project's tests, CC0, matching the
    repo convention. Keep each fixture under 20 KiB.
  - The test is that `read` accepts each fixture.
  - The other direction is opt-in: when `SR_CORE_DIR` is set, `pack` output is checked with
    `python3 -m sr_core.vpkg verify`. If you can't run Python from a Rust test cleanly, make it a documented
    `#[ignore]` test.
- Hostile tests, each returning `Err`:
  - truncated
  - no EOCD
  - bad CRC
  - zip bomb, where the advertised size is small and the real output is large
  - duplicate and case-duplicate names
  - `../x`, `/x` and `C:x`
  - symlink
  - ZIP64 marker
  - data past the central directory
  - stray entry
  - hash mismatch
  - wrong first entry
  - two primary scenes
  - pipeline cycle
  - major 2
  - a 1.1 package with a wrong metaSha256
  - a missing referenced asset
- wasm: the crate must build for wasm32. Add it to `cargo xtask wasm` only if the xtask derives the L0–L4 list itself;
  check how it does.

## Gate (VM only, through `vm-agent.sh vpkg …`)
```
cargo clippy -p filmcraft-vpkg --all-targets -- -D warnings && cargo test -p filmcraft-vpkg && cargo xtask layers && cargo xtask assets && cargo xtask wasm
```
Don't commit. Reply HANDOFF with:
- the public API
- the list of borrowed files
- the red→green log
- the gate result

# FIX ROUND 1 (continuation of your task above)
Your work is still uncommitted in this worktree. An independent review found the defects below. Fix them test-first: for each, add a failing test, then the fix. Then re-run the full gate on the VM.

1. `zip.rs:199` decodes every file name as UTF-8 and ignores general-purpose flag bit 11. Reject any entry whose name has a non-ASCII byte when bit 11 is clear, in both the central and local headers. The writer must set bit 11 when a name is non-ASCII. Test: `project/\xC3\xA9.scene` with bit 11 clear is rejected, and the same name with bit 11 set is accepted.
2. `zip.rs:108`: the case-collision key `to_uppercase()` misses `ß`/`ẞ`. Use `name.to_lowercase().to_uppercase()` as the key (std only, no new crates), both for the collision check and for the parent-prefix check. Test: `project/ß.png` + `project/ẞ.png` is rejected.
3. `verify.rs:284` and `verify.rs:361` only strip an XML declaration written `<?xml ` with a space. Accept any XML whitespace (space, tab, CR, LF) after `<?xml`, and put that in one bounded helper shared by the includes and MaterialX paths. Test: an included `inc.xml` whose declaration is `<?xml\nversion="1.0"?>`, referencing a packaged `data.json`, verifies.
4. `write.rs:154`: the written package embeds the MIT-licensed sr-core schema without its notice. Add a short section to the generated README.md (before `metaSha256` is computed) naming `vpkg.schema.json` as from sr-core, with its MIT copyright and permission notice, copied from sr-core's LICENSE. Keep it deterministic. Test: the README of `pack()` output contains the notice, and `metaSha256` still verifies.
5. Don't change the code for Unicode decimal digits in `%0Nd`/`#` sequence matching. Document it in crates/vpkg/README.md as a known divergence from refs.py: ASCII digits only, so stricter.

Append the red→green results to TDD.md. Gate (VM, through `vm-agent.sh vpkg …`):
`cargo fmt --check -p filmcraft-vpkg && cargo clippy -p filmcraft-vpkg --all-targets -- -D warnings && cargo test -p filmcraft-vpkg && cargo xtask layers && cargo xtask assets && cargo xtask wasm`.
Re-run the Python oracle (`sr_core.vpkg verify` on a Rust-written package, including one with a non-ASCII name). Reply HANDOFF with each fix's test name, the red→green result, and the gate result.
