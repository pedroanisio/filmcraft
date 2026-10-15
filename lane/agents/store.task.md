**B3a: the disk frame cache and the placeholder slate.** Scope: `src/store.rs` and `src/placeholder.rs`.

1. **`store`** keeps frames as rendered PNG files under the media cache directory. Read
   `crates/engine/src/settings.rs` (`media_cache_dir`) and `crates/engine/src/previews.rs` (`PreviewStore`: temp then
   rename) for the house style.
   - `pub struct AssetStamp { pub reference: String, pub len: Option<u64>, pub modified_ns: Option<i128> }`. None means
     missing or unreadable.
   - `pub fn stamp_assets(base_dir: &Path, refs: &[String]) -> Vec<AssetStamp>` resolves each ref relative to
     `base_dir`, the way scene-render does (read `resolve`/`inside` in
     `/home/admin/src/scene-render/crates/sr-resolve/src/lib.rs`): `file://` URLs, `%`-escapes, `..`. Remote URLs and
     unresolvable refs get None stamps. It is bounded in count, and it is native only (wasm gives all None).
   - `pub fn key(renderer_version: &str, quality: Quality, doc: &[u8], assets: &[AssetStamp]) -> String` returns 64
     lowercase hex characters, `filmcraft_bitstream::sha256` over an **unambiguous** framing (a domain tag, a
     version, and a length prefix on every field). Tests check that each input changes the key, and that field
     boundaries cannot collide (`("ab","c")` vs `("a","bc")`).
   - `pub struct Store` is created with `Store::new(media_cache_dir: &Path)`; its root is `<dir>/Scene Renders`.
     - `frame_path(key, quality, k) -> filmcraft_media::Result<PathBuf>` gives `<root>/<key>/<quality>/<k:06>.png`.
       Scene-render's `%06d` grows past 6 digits; match that. The key is validated as exactly 64 hex characters, so
       nothing can escape the root.
     - `has_frame`.
     - `begin_chunk(key, quality) -> Result<ChunkDir>` creates a fresh unique temp dir `<root>/<key>/.tmp-<pid>-<n>`.
     - `ChunkDir::pattern() -> PathBuf` is `<tmp>/%06d.png`, passed to scene-render `-o`.
     - `ChunkDir::commit(frames: Range<u64>, validate: impl Fn(&Path) -> filmcraft_media::Result<()>) -> filmcraft_media::Result<()>`
       validates every expected frame file and then renames each into place. If any frame is missing or invalid, it
       commits nothing.
     - Drop removes the temp dir. Every error is propagated, never panicked on.
     - `clear(&self) -> filmcraft_media::Result<u64>` removes the whole root and returns the bytes freed.
     - `size(&self) -> u64`.
     - Two `Store`s on the same root used concurrently from threads must never produce a torn frame. Test it.
   - Native only; on wasm every operation returns `MediaError::Unsupported`.
2. **`placeholder`**: `pub fn slate(width: u32, height: u32) -> filmcraft_media::Result<filmcraft_frame::VideoFrame>`.
   - A deterministic, clearly not-rendered image: diagonal stripes in two dark greys with a muted amber band.
   - Opaque, straight-alpha sRGB RGBA8, built the way `filmcraft_media::generators` builds RGBA8 frames.
   - The allocation is checked: refuse more than 16384 per side or an overflowing product.
   - Tests: determinism, size, opacity, overflow refusal, zero size.

Hostile tests:
- a key with `../`;
- a non-hex key;
- `k = u64::MAX`;
- a root that is a file;
- a read-only root, under unix.
