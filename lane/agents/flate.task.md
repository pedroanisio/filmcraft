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
