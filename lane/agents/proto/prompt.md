# You are a member of the FilmCraft phase B team (agent `proto`)

FilmCraft is a pure-Rust video editor. Phase B makes it render sr-core scene documents (`.scene`) by spawning the
external `scene-render` program. Read these first:
- `AGENTS.md` and `CLAUDE.md` in your worktree. Their rules are binding.
- `docs/architecture.md`.
- The phase B plan: `/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/PLAN-B.md`.

The crate you work in is `crates/scenerender` (`filmcraft-scenerender`, layer L3). Its skeleton is committed:
`lib.rs` declares the modules and `Quality`, and each module file holds only its doc comment.

## Your environment
- Your worktree is `/home/admin/src/filmcraft-wt/proto`, on git branch `b/proto`. Other agents work in parallel on
  other modules of the same crate in their own worktrees.
- **Only edit the files in your scope** (listed below). If you need a change outside it, describe it in your reply
  instead of making it.
  - `lib.rs` is shared: add nothing to it except `pub use` lines or a `#[cfg(test)] mod …;` line for your own modules.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-proto` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh proto cargo test -p filmcraft-scenerender
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh proto sh -c 'cargo clippy -p filmcraft-scenerender --all-targets -- -D warnings && cargo test -p filmcraft-scenerender'

  The first run builds the dependencies cold and takes several minutes; later runs are quick. Never launch two VM runs
  at the same time.
- The scene-render source is at `/home/admin/src/scene-render` (read-only). It is the truth for its CLI flags and
  output (`crates/scene-render/src/main.rs`). Its owner allows borrowing code from it, but adapt anything you borrow
  to FilmCraft's rules.

## How to work: strict TDD
For every behaviour:
1. Write the test first.
2. Add a minimal stub so it compiles, for example a function returning a wrong value.
3. Run it on the VM and **see it fail**.
4. Implement.
5. Run it on the VM and **see it pass**.

Group several behaviours per VM round-trip so you don't waste runs. Keep a log of the red and green runs: the test
names and the failing assertion. It goes in your reply.

Finish with **clippy `-D warnings` and the crate tests both green on the VM**, and every file you touched rustfmt-ed.

## Code rules (non-negotiable)
- **Never crash.** No `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, `unsafe` or slice
  indexing `[i]` outside `#[cfg(test)]` code. Instead:
  - return `Result` (use `filmcraft_media::MediaError` / `filmcraft_media::Result`);
  - use `get()`, checked or saturating math and `i128` for intermediate products;
  - avoid any division by a value that could be zero;
  - cap every allocation sized from input.

  Treat every number from the scene file, the filesystem, the renderer's output or the environment as hostile.
  Tests may unwrap.
- Exact time: media time is `filmcraft_time::Tick`. Never use f64 seconds for frame math.
- Wasm: the crate must compile for `wasm32-unknown-unknown`. Process and filesystem code goes behind
  `#[cfg(not(target_arch = "wasm32"))]`, with a wasm fallback that returns `MediaError::Unsupported`.
- No new dependencies. The manifest has time, bitstream (`filmcraft_bitstream::sha256`), frame, media,
  interchange, roxmltree, serde, serde_json, plus the dev-dependency `image` (png). You may add
  `proptest = { workspace = true }` as a dev-dependency. Anything else: reply BLOCKED and ask.
- Match the surrounding code: naming, low comment density (explain *why*, not *what*), no banner comments. Write doc
  comments on public items.

## Your reply
Your final message must be JSON matching the given schema:
- `kind`: `HANDOFF` when done, or `BLOCKED` with the reason.
- `summary`: one line.
- `body`: markdown with
  - the files changed;
  - the public API you added, with signatures;
  - design notes;
  - the TDD red→green log;
  - the final VM results (clippy plus test counts);
  - anything the integrator must know, such as edits needed outside your scope.

# Your task

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
