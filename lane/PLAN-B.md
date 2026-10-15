# FilmCraft ↔ scene-render external renderer (phase B)

Branch `scene-interchange`. Tasks B0–B6 are one commit each with `cargo xtask ci` green on fc-build-01; B7 waits for A's S5–S7.
Phase B adds **no new workspace dependencies**:
- PNG decoding: `filmcraft_media::still::decode_raster` (the `image` crate, already a dependency)
- WAV reading: `filmcraft_media::wav`
- cache keys: `filmcraft_bitstream::sha256` (S1)

## What it does
A `.scene` file (an sr-core scene document) imports as a **rendered media item**: one clip whose frames and audio come from
scene-render, an external program that is installed and licensed separately. FilmCraft never links or ships scene-render; it only
spawns it, the way an OS codec is a runtime option. Playback, render previews and export all reach the frames through the normal
`MediaSource` path (`MediaPool` → `render::SourceProvider` → `MediaSource::video_frame`), so nothing else needs wiring.

Without the renderer, the item still imports with the right size, fps and duration from FilmCraft's own parse of the header. It
shows a **placeholder** slate, and the import result and `scene.renderer.status` both say why.

## scene-render contract (0.5.0)
- `scene-render render DOC --frames A..B -o DIR/%06d.png --bit-depth 8 --quality draft|preview|final`
  - The frame range is end-exclusive.
  - Output is 8-bit straight-alpha sRGB RGBA PNG, one file per frame, numbered by **project frame index**.
  - Frame k is at t = k·den/num.
  - Exit codes: 0 ok, 1 invalid document (diagnostics on stdout/stderr), 2 usage, I/O or GPU error.
  - There is no stdout frame output, no resolution or fps override, and no server mode.
  - Assets resolve relative to the document's directory.
- **Frame count:** `ceil(duration·num/den)`, where a value within 1e-9 of an integer rounds (`sr-model values.rs:291`). The tick form is exact: `ceil(d_ticks·num / (den·TICKS))`.
- **Audio:** `scene-render encode DOC -o OUT.wav` writes the WAV mixdown natively (`sr_audio::wav`), with no ffmpeg. A document with no audio fails with "audio-only output, but the scene has no audio", which means silence.
- **Probes:** `--version` prints `scene-render X.Y.Z`. `capabilities` prints JSON with `"format": "scene-render-capabilities/1"`, `engine.version` and `schema`.
- **Runtime needs:** a GPU through wgpu (lavapipe works) and system fonts. Time for one invocation is dominated by GPU and process start-up, so frames render in **chunks**.

## Crate `filmcraft-scenerender` (L3, new, `crates/scenerender`)
Dependencies: time, frame, media, interchange (`scene::read` / `values` for the header) and bitstream (sha256). It is registered in
`xtask` LAYERS as L3 and in `[workspace.dependencies]`. Process code is `#[cfg(not(target_arch = "wasm32"))]`; on wasm the
renderer is always "unavailable", so the placeholder is used.

| module | task | contents |
|---|---|---|
| `header` | B1 | `SceneHeader{width,height,fps: FrameRate,duration: Tick}` from `<project>` via `interchange::scene::{read,values}`; `frame_count()` (exact rule above); `has_audio_assets` (any `<audio>`, or a `<video>` that can carry audio); `asset_refs()` (every `src`/`href` value, bounded) |
| `timing` | B1 | `frame_at(t, fps, count) -> u64` (floor, clamped to the last frame, negative → 0, checked i128); `frame_time(k, fps) -> Tick`; `chunk(k, len, count) -> Range<u64>` aligned to `len` |
| `discover` | B1 | `Config{program: Option<PathBuf>, …}`. `locate(config, env, is_file)`: explicit pref → env `FILMCRAFT_SCENE_RENDER` → `PATH` search for `scene-render[.exe]`. `parse_version` (min 0.5.0), `parse_capabilities` (format tag, engine version, schema) |
| `args` | B1 | `render_args(doc, frames, pattern, quality)`, `audio_args(doc, wav)`. The doc path is made absolute so it can never be read as a flag. Built as `OsString`s; never a shell |
| `process` | B2 | the `Runner` trait (`run(program, args, cwd, deadline, cancel) -> Result<RunOutput{code, stderr_tail}>`). `ProcessRunner` has null stdin, piped stdout and stderr drained by threads into bounded tails (64 KiB), `try_wait` polling, and kill + reap on deadline or cancel (`media::cancel::cancelled`). Exit 1 → `MediaError::Decode(diag)`, 2/other → `Io` |
| `ingest` | B2 | reads one rendered PNG (size cap, `decode_raster`), checks that its dimensions equal the header's, returns `VideoFrame::rgba8` (straight sRGB) with `color` set to sRGB/709 |
| `store` | B3 | the disk cache: `key = sha256(renderer version ‖ quality ‖ doc bytes ‖ for each asset ref: resolved path, len, mtime)`, stored at `<media cache>/Scene Renders/<key>/<quality>/<k:06>.png`. A chunk renders into a fresh temp dir, and each file is renamed in only after the run succeeds and its PNG checks out. `clear(root)` |
| `placeholder` | B3 | a deterministic slate (diagonal stripes, header-sized) |
| `source` | B3 | `SceneRenderSource: MediaSource`. Lookup order: an in-memory `FrameCache`, then the disk store, then a chunk render. Rendering is serialised per source, and a global limit allows one renderer process at a time, so concurrent requests for the same chunk do not render it twice. Quality: `media::cancel::draft()` → draft, otherwise preview for playback and **final** when the pool is in export mode. `audio()` (B5) |
| `lib` | B0 | `open(path, bytes, &Env) -> Result<SharedSource>` (picks real or placeholder), `Status` / `probe(&Config)` (cached per config) |

## Engine wiring (B4)
- **Import:** `.scene` joins the importable extensions as a media file. `MediaRef::File{path}` stays unchanged, so projects stay
  compatible.
  - `MediaPool::full_res_source` / `source_for` send `.scene` paths to `filmcraft_scenerender::open` along with the prefs config and
    the media cache dir. Changing the renderer pref drops the cached scene sources.
  - `file.import` adds a warning to its result when the renderer is missing.
  - A's S8 later adds `as: "sequence"` (interchange import); the default stays `"media"` unless the owner decides otherwise.
- **Prefs:** `PluginPrefs.scene_render_path: String` (`Kind::Path`, empty = auto) and `scene_render_preview_quality: draft|preview`,
  both in `categories()`.
- **Commands:**
  - `scene.renderer.status`: `{found, path, version, capabilities, reason}`
  - `scene.renderer.clearCache`
  - both with hostile-parameter tests, and both in `docs/control-protocol.md`
- **Export:** goes through the pool provider in export mode, so it renders at `final` quality. A placeholder in an export is reported
  in the export result.

## Tests (TDD: every behaviour starts as a failing test)
- **Pure tests** cover header, timing, discover, args and the store key/layout.
- **`process` / `source` tests use a fake renderer.** It is a `#[cfg(unix)]` sh script written into a temp dir at test time. It
  parses `--frames A..B -o PATTERN` and copies a template PNG (made by the test with `image`) to each frame path. Variants exit 1/2,
  write a wrong-size PNG, sleep past the deadline, or write nothing. None of them may panic.
- **Opt-in oracle:** when `SR_RENDER_BIN` is set, render a conformance case and check size, frame count and a non-blank frame. It is
  never required in CI.
- **Engine tests:** importing a `.scene` with no renderer gives the placeholder and a warning; with the fake renderer, frames come back
  through the pool; hostile command params.

## Tasks
- **B0** Crate skeleton: Cargo manifest, layers entry, workspace dep, module stubs with the public types above. ci green.
- **B1** `header`, `timing`, `discover`, `args`: pure, with unit and hostile tests.
- **B2** `process` + `ingest`, with the fake-renderer harness (unix).
- **B3** `store`, `placeholder`, `source` (video).
- **B4** Engine wiring: import, pool, prefs, commands, docs/control-protocol.md.
- **B5** Audio: `encode -o mix.wav` once per (doc key), then `WavSource`, with silence when the scene has none or audio fails (reported).
- **B6** Docs: `docs/scene-render.md` (install, discovery order, cache, licence note: scene-render is separate software, never shipped),
  `docs/gaps.md`, `ROADMAP.md`. Also the opt-in `SR_RENDER_BIN` oracle test, run once on the VM if scene-render can be built there.
- **B7** (after S5–S7) Hybrid rendering. Subtrees that A cannot map (3D, particles, physics, expressions) export as a sub-document,
  render through B as an alpha overlay clip, and go on the track above their paint position.

## Team (Codex agents via govern lane jobs)
- Each agent runs in its own worktree `/home/admin/src/filmcraft-wt/<name>`, on branch `b/<name>` off `scene-interchange`.
- It runs as a lane job `codex-<name>` (`codex-agent.sh <name>`, workspace-write, network on for the VM).
- It tests **only** through `vm-agent.sh <name> CMD`, which uses VM dir `filmcraft-<name>`; there is never a local cargo.
- It does not commit. Claude reviews, merges into `scene-interchange`, gates with `ci-head`, and commits one task per commit.
- **Parallel wave 1** (after B0): `proto` (B1), `runner` (B2), `store` (B3 store and placeholder). **Wave 2:** `source` (B3 source and B5). **Wave 3:** `engine` (B4). B6 is mine.
