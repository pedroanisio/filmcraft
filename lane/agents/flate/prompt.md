# You are a member of the FilmCraft scene-interchange team (agent `flate`)

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
- Your worktree is `/home/admin/src/filmcraft-wt/flate`, on git branch `b/flate`. Other agents work in parallel in
  their own worktrees.
- **Only edit the files in your scope** (listed in your task). If you need a change outside it, describe it in your
  reply instead of making it.
  - `scene/mod.rs` is shared: add only your `pub mod …;` and `pub use …;` lines to it.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-flate` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh flate cargo test -p filmcraft-interchange scene
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh flate sh -c 'cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm'

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

**S9: move raw inflate and deflate into `filmcraft-bitstream::flate` (L0, public). `filmcraft-media` and `filmcraft-export` then
use it. No behaviour change.**

This task is not in `scene/`. It is a refactor so that S10 (`filmcraft-vpkg`, ZIP) can use one inflater and deflater without an
L3→L3 dependency. Ignore the scene-specific paths in the "environment" section above; this section gives your scope.

## What exists
- `crates/media/src/still/exr/inflate.rs` (private): `zlib(src, expected)`, `inflate(src, limit)`, a bounded fast-table Huffman
  decoder, `Error = &'static str`. Used by `crates/media/src/still/exr.rs`.
- `crates/export/src/deflate.rs` (private): `zlib(data)` (an LZ77 + dynamic/fixed/stored block encoder, adler32), and a
  `#[cfg(test)] pub(crate) mod tests` with a test inflater used by `crates/export/src/raster.rs` tests.
- `filmcraft-bitstream` already has `crc32` and `sha256` and no dependencies.

## Scope (you may edit)
- `crates/bitstream/src/flate.rs` (new; a `flate/` directory is fine) and the `pub mod flate;` line plus module doc line in
  `crates/bitstream/src/lib.rs`.
- `crates/media/src/still/exr/inflate.rs` (delete it, or make it a thin re-export) and the call sites in `crates/media/src/still/exr.rs`.
- `crates/export/src/deflate.rs` (delete or thin) and its call sites in `crates/export/src/raster.rs` (tests included).
- `crates/media/Cargo.toml` and `crates/export/Cargo.toml`: add **only** `filmcraft-bitstream = { workspace = true }`. Check how
  other crates reference it; it's an internal path crate, not a new external dependency.
- `ATTRIBUTION.md`: nothing should be needed, since this is our own code moving.

## Public API (in `filmcraft_bitstream::flate`; doc comments required)
```rust
pub enum FlateError { … }   // thiserror isn't a bitstream dep: implement Display + std::error::Error by hand, or keep &'static str messages inside it
pub fn inflate(src: &[u8], limit: usize) -> Result<Vec<u8>, FlateError>;        // raw DEFLATE (RFC 1951), output capped at limit
pub fn inflate_exact(src: &[u8], expected: usize) -> Result<Vec<u8>, FlateError>; // raw, output must be exactly `expected` (ZIP: uncompressed size)
pub fn zlib_decode(src: &[u8], expected: usize) -> Result<Vec<u8>, FlateError>; // RFC 1950 wrapper + adler32 check (what exr uses today)
pub fn deflate(data: &[u8]) -> Vec<u8>;                                          // raw DEFLATE stream, the existing encoder
pub fn zlib_encode(data: &[u8]) -> Vec<u8>;                                      // what export uses today, byte-identical output
pub fn adler32(data: &[u8]) -> u32;
```
Also report how many bytes of `src` a raw inflate consumed, because ZIP needs to check that the compressed size is used exactly.
Either add `inflate_with_consumed(src, limit) -> Result<(Vec<u8>, usize), FlateError>` or return it some other way, and document it.

## Requirements
- **No behaviour change**:
  - every existing EXR read test and EXR/PNG write test stays green with the same expected bytes;
  - `zlib_encode` output is byte-identical to today's `export::deflate::zlib` (pin it with a test on a few inputs before the move);
  - media keeps its current error messages wherever a test asserts on them.
- **Never crash**, and hostile input must never panic: truncated streams, bad block type 3, over-subscribed or incomplete Huffman
  codes, distance before the start of the output, a stored-block LEN/NLEN mismatch, output past `limit`, trailing garbage. The
  limit is checked before growing the output.
- Bitstream is L0 and must build for wasm32 with zero dependencies.
- Deduplicate: one copy of the length/distance tables.
- Run the whole thing over the `crates/media/src/still/exr*` and `crates/export` test suites.

## TDD
- **Before moving anything**, write `crates/bitstream` tests:
  - round-trip `inflate(deflate(x)) == x` over proptest inputs, including the empty input, 1 byte, 64 KiB+1, highly repetitive
    data and random data;
  - RFC 1951 hand-made vectors: one stored block, one fixed-Huffman block, one dynamic block. Write the bytes by hand, with a
    comment working them out;
  - the zlib header and adler32 vectors (`adler32(b"Wikipedia") == 0x11E60398`);
  - the hostile cases above, each returning `Err`;
  - the consumed-bytes count, including trailing data after the final block.
- See them fail against stubs, then make them green.
- Then move the code, and keep the media and export suites green.

## Gate (VM only)
```
cargo clippy -p filmcraft-bitstream -p filmcraft-media -p filmcraft-export --all-targets -- -D warnings && cargo test -p filmcraft-bitstream -p filmcraft-media -p filmcraft-export && cargo xtask layers && cargo xtask wasm
```
Use `vm-agent.sh flate <cmd>`. Don't commit. Reply HANDOFF with the gate result.
