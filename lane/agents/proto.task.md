**B1: the pure protocol layer.** Scope: `src/header.rs`, `src/timing.rs`, `src/discover.rs`, `src/args.rs`, and
`proptest` as a dev-dependency if you want it. Nothing here spawns processes or touches the filesystem, except
`discover::locate_system`, which reads the real environment.

1. **`header`** is the scene facts FilmCraft needs without the renderer.

   `pub struct SceneHeader { pub width: u32, pub height: u32, pub fps: FrameRate, pub duration: Tick, pub has_audio: bool, pub asset_refs: Vec<String> }`

   `pub fn parse(bytes: &[u8]) -> filmcraft_media::Result<SceneHeader>`
   - Use `filmcraft_interchange::scene::read::{decode, read, Limits}` and `scene::values`. Read
     `crates/interchange/src/scene/` and the vendored XSD `crates/interchange/schema/sr-core/scene-render.xsd` for
     the `<project>` attributes and their types.
   - Map errors to `MediaError::Decode` with a useful message.
   - Reject zero or absurd sizes. Pick a documented cap, e.g. 16384 per side.
   - `has_audio` is a conservative superset: true when the document has any asset or element that can produce sound
     (look at the XSD: audio assets, video assets, generated speech, …).
   - `asset_refs` lists every attribute value that references a file (look in the XSD: `src`, `href`, …), in
     document order, deduplicated, and bounded in count and length.
   - `pub fn frame_count(&self) -> u64` matches scene-render exactly:
     - see `/home/admin/src/scene-render/crates/sr-model/src/values.rs:291`, `ceil(duration·num/den)`, where a value
       within 1e-9 of an integer rounds;
     - compute it in exact integer tick math;
     - check the rounding consequences of FilmCraft's seconds→Tick conversion, and test with rates 24, 25, 30000/1001
       and 60, plus durations that are and aren't frame multiples.
2. **`timing`**: `frame_count(duration: Tick, fps: FrameRate) -> u64` (used by `header`).
   - `frame_at(t: Tick, fps: FrameRate, count: u64) -> u64`: floor; negative becomes 0; clamped to `count-1`;
     `count == 0` gives 0.
   - `frame_time(k: u64, fps: FrameRate) -> Option<Tick>`: the exact start of frame k, k·den/num s.
   - `chunk(k: u64, len: u64, count: u64) -> Range<u64>`: the aligned chunk containing k, clipped to `count`, never
     empty when `count > 0`; a `len` of 0 is treated as 1.
   - Everything uses checked `i128`. A rate with num ≤ 0 or den ≤ 0 must not panic: return 0 or None.
   - Add property tests that `frame_at(frame_time(k)) == k` and that frame_at is monotonic.
3. **`discover`**:
   - `pub struct Config { pub program: Option<PathBuf> }` (serde, Default).
   - `pub fn locate(config: &Config, env: impl Fn(&str) -> Option<OsString>, is_file: impl Fn(&Path) -> bool) -> Option<PathBuf>`:
     1. the explicit program if it is a file;
     2. otherwise env `FILMCRAFT_SCENE_RENDER` if it is a file;
     3. otherwise search `PATH` (`std::env::split_paths`) for `scene-render`, or `scene-render.exe` on Windows.
   - `locate_system(config)` uses the real env and fs; on wasm it returns None.
   - `pub struct Version { major, minor, patch }`, `pub const MIN_VERSION`.
   - `parse_version(stdout: &str) -> Option<Version>` for the line `scene-render X.Y.Z`. Tolerate trailing text and a
     newline, and bound the input.
   - `pub struct Capabilities { pub engine_version: String, pub schema: String }`.
     `parse_capabilities(json: &str) -> Result<Capabilities, String>` requires `"format": "scene-render-capabilities/1"`
     (see `/home/admin/src/scene-render/capabilities.json`) and caps the input at 8 MiB.
   - `pub struct Status { pub found: bool, pub path: Option<String>, pub version: Option<String>, pub schema: Option<String>, pub reason: Option<String> }`
     (serde camelCase). This is what the `scene.renderer.status` command will return.
4. **`args`**: argument vectors as `Vec<OsString>`, never a shell string. Check the exact flag spelling against
   scene-render's clap definitions.
   - `render_args(doc: &Path, frames: Range<u64>, pattern: &Path, quality: Quality) -> filmcraft_media::Result<Vec<OsString>>`
     gives `render DOC --frames A..B -o PATTERN --bit-depth 8 --quality q`.
     - `doc` and `pattern` must be absolute; otherwise return `MediaError::Unsupported`. Absolute paths can never be
       taken for flags.
     - The frame range must be non-empty and within u64 limits.
   - `audio_args(doc, wav) -> …` gives `encode DOC -o WAV`. Check how scene-render's `encode` picks audio-only output
     from the `.wav` extension, and what else it needs.
   - `version_args()` and `capabilities_args()`.

Hostile tests:
- XML with a huge width;
- a missing `<project>`;
- fps `0/1`;
- an empty or garbage `--version` output;
- a capabilities JSON that is truncated, has the wrong format tag or is gigantic;
- a relative doc path.
