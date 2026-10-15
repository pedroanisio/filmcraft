# You are a member of the FilmCraft phase B team (agent `source`)

FilmCraft is a pure-Rust video editor. Phase B makes it render sr-core scene documents (`.scene`) by spawning the
external `scene-render` program. Read these first:
- `AGENTS.md` and `CLAUDE.md` in your worktree. Their rules are binding.
- `docs/architecture.md`.
- The phase B plan: `/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/PLAN-B.md`.

The crate you work in is `crates/scenerender` (`filmcraft-scenerender`, layer L3). Its skeleton is committed:
`lib.rs` declares the modules and `Quality`, and each module file holds only its doc comment.

## Your environment
- Your worktree is `/home/admin/src/filmcraft-wt/source`, on git branch `b/source`. Other agents work in parallel on
  other modules of the same crate in their own worktrees.
- **Only edit the files in your scope** (listed below). If you need a change outside it, describe it in your reply
  instead of making it.
  - `lib.rs` is shared: add nothing to it except `pub use` lines or a `#[cfg(test)] mod …;` line for your own modules.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-source` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh source cargo test -p filmcraft-scenerender
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh source sh -c 'cargo clippy -p filmcraft-scenerender --all-targets -- -D warnings && cargo test -p filmcraft-scenerender'

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

## Wave 2 overrides (these supersede the shared rules above where they differ)
- Wave 1 is merged and committed (B1 `8e0b5076`, B2 `5d1d961a`, B3a `e7dac102`). The module files are **not** empty
  any more: `header`, `timing`, `discover`, `args`, `process`, `ingest`, `fake` (test only), `store` and
  `placeholder` are implemented and tested. Treat them as read-only, with one exception: if a real bug in one of them
  blocks you, you may fix it with a regression test, and you must say so in your reply.
- You are the only agent working right now. You **may restructure `lib.rs`** as your task describes (keep the
  module list and `Quality`).
- You may add `log = { workspace = true }` to `[dependencies]` in `crates/scenerender/Cargo.toml`. `log` is already a
  workspace dependency, so it is not a new one. Tell the integrator, who updates Cargo.lock; the VM build updates it
  for your runs anyway.
- Your VM checkout `~/filmcraft-source` is seeded with a built `target/`, so the first run is warm.

**B3b + B5: the `MediaSource` over scene-render, video and audio.**

Scope:
- `src/source.rs`
- `src/lib.rs`: the `open` / `probe` entry points and re-exports. You may restructure `lib.rs`, but keep the existing
  module list and `Quality`.
- New test files under `src/` if you want them, declared `#[cfg(test)]`.

The building blocks are already merged; **read them first and use them, don't duplicate them**:
- `header`: `SceneHeader::parse`, `frame_count`, `has_audio`, `asset_refs`
- `timing`: `frame_at`, `frame_time`, `chunk`
- `discover`: `Config`, `locate`, `parse_version`, `parse_capabilities`, `Status`
- `args`: `render_args`, `audio_args`, `version_args`, `capabilities_args`
- `process`: `Runner`, `ProcessRunner`, `check`
- `ingest`: `read_frame`
- `store`: `key`, `stamp_assets`, `Store`, `ChunkDir`
- `placeholder`: `slate`
- `fake` (test only): `FakeRenderer`

The public API that the engine (B4) will call:

```rust
pub struct Env {
    pub config: discover::Config,
    pub cache_dir: Option<PathBuf>,   // media cache dir; None → no disk cache (memory only, render to a temp dir)
    pub quality: Quality,             // quality for non-draft requests (Preview for playback; the engine opens a
                                      // second source with Final for export)
    pub chunk_len: u64,               // frames per renderer invocation (default 48)
    pub timeout: Duration,            // per invocation (default 10 min)
    pub memory_budget: usize,         // FrameCache bytes (default 256 MiB)
}
impl Default for Env { … }

pub struct Opened { pub source: SharedSource, pub status: discover::Status, pub warning: Option<String> }

/// path: the .scene file (absolute; its directory is the asset base); bytes: its contents.
pub fn open(path: &Path, bytes: &[u8], env: &Env) -> filmcraft_media::Result<Opened>;
/// Same, with an injected runner and an already-located program (tests and the engine's fake-renderer tests).
pub fn open_with(path: &Path, bytes: &[u8], env: &Env, program: Option<PathBuf>, runner: Arc<dyn process::Runner>) -> filmcraft_media::Result<Opened>;
/// Locate and probe the renderer (version ≥ MIN_VERSION, capabilities). Cached per resolved program path + file
/// len/mtime in a process-wide map, so it's cheap to call repeatedly.
pub fn probe(config: &discover::Config) -> discover::Status;
```

Behaviour:
1. **Info comes from the header alone, identical with and without the renderer**, so a project's media info never
   changes when the renderer appears or disappears.
   - Build the `MediaInfo` exactly as other sources do (see `still.rs` and `wav.rs` in `crates/media`): one video
     stream with the header's size, fps and duration, `container: "sr-core scene"`, and `file_size`.
   - Add one audio stream when `header.has_audio`. Find the rate and channel count scene-render's `encode` writes for
     WAV: look in `/home/admin/src/scene-render/crates/` (`sr-audio`, and the encode pipeline in the `scene-render`
     crate), and declare that rate and channel count.
   - An invalid header is an `Err(MediaError::Decode)` from `open`. A missing renderer is not an error.
2. **Renderer missing or unusable** (not found, version too old, probe failed, or wasm): the source is a
   `PlaceholderSource`.
   - `video_frame` returns the cached `slate`, scaled down when `req.scale < 1` if that is cheap; otherwise full size
     is fine.
   - `audio` returns silence.
   - `Opened.warning` and `Opened.status.reason` say why, e.g. "scene-render not found: set Preferences › Plugins ›
     scene-render, FILMCRAFT_SCENE_RENDER or PATH".
3. **Renderer available**: `SceneRenderSource`.
   - **Video:** `video_frame(req)` maps `k = frame_at(req.time, fps, count)` and picks the quality:
     `filmcraft_media::cancel::draft()` gives `Draft`, otherwise `env.quality`. The lookup order is:
     1. the in-memory `FrameCache<(Quality, u64)>`;
     2. the disk `Store` (`key` covers renderer version, quality, doc bytes and asset stamps; compute the stamps
        once at open);
     3. render the aligned `chunk(k, chunk_len, count)`. Only render the frames of the chunk missing on disk: trim
        the range to the missing span. Use a `ChunkDir`, `render_args`, run, `check`, then `commit` with an
        `ingest::read_frame`-based validator, then read frame k.
   - **Concurrency:**
     - A per-source mutex serialises rendering, and a process-wide limit allows one renderer process at a time.
     - After acquiring the lock, look on disk again, so two threads asking for the same chunk render it once. Test
       it with threads and the fake's invocation log.
     - Lock poisoning is handled with `PoisonError::into_inner`.
   - **Cancellation:** `cancel::cancelled()` before starting gives `Err(Cancelled)`. The runner kills the child on
     cancel, and a cancelled chunk commits nothing.
   - **Without `cache_dir`:** render into a unique temp dir under `std::env::temp_dir()` and clean it up. Keep
     frames only in memory.
   - **Errors:** a renderer failure or a bad frame is an `Err` from `video_frame` with the diagnostic. The engine's
     pool turns that into its slate. Never panic, and never cache a failure on disk. In memory, remember a
     failed chunk for that quality so playback doesn't respawn the renderer 24×/s: `Err` until the source is
     reopened.
   - **Audio (B5):** on the first `audio()` call, render the mixdown once with `encode DOC -o <store>/<key>/mix.wav`
     (key with a fixed quality tag, e.g. `Final`; audio doesn't depend on quality), using temp then rename. Then
     serve it through `filmcraft_media::wav::WavSource`: check `parse` vs `open` (a streaming reader) and use the
     streaming one if it exists, so big mixes aren't held in memory. Check how `WavSource::audio` handles a
     `sample_rate` different from the file's, and match it.
     - "audio-only output, but the scene has no audio", or any failure, gives silence, remembered so encode runs
       once, and logged once with `log::warn!` if `log` is already a workspace dependency. **Don't add it if it isn't;
       store the reason instead.**
     - `audio()` must never return `Err` for render failures: silence is the fallback. It should honour the
       `frames` count and the channel layout declared in info.
     - Out-of-range `start` (negative, past the end) gives silence. Cap `frames` sensibly.
4. **Hostile inputs:**
   - huge `req.time`, negative time, NaN/∞ `scale`;
   - a renderer that writes wrong-size or zero frames, exits 1 or 2, or hangs (use a short `timeout` in `Env` in
     tests);
   - a cache dir that is a file;
   - a doc whose header says 1 frame;
   - `chunk_len = 0`.
5. **Tests (unix, with `FakeRenderer`):**
   - frames come back at the right size;
   - the second request comes from memory (no new invocation);
   - a new source on the same cache dir hits disk (no invocation);
   - quality change → separate renders;
   - draft hint → `--quality draft`;
   - two threads → one render;
   - failure → `Err` and no disk files;
   - placeholder when the program is None, plus a warning;
   - audio mix served and resampled;
   - no-audio → silence and a single invocation;
   - `probe` with the fake → found + version.

   One test proves info is identical between placeholder and real sources.

Add an **opt-in oracle test** gated on env `SR_RENDER_BIN` (skip silently when unset):
- open a tiny scene (64×64, 24 fps, 0.5 s, one coloured rect shape; see `/home/admin/src/sr-core` conformance cases
  for valid minimal documents) with the real renderer;
- check frame count, size, that a frame isn't blank, and that a second open hits disk.
