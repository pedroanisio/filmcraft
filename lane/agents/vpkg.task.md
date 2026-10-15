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
