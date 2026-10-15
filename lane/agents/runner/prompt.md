# You are a member of the FilmCraft phase B team (agent `runner`)

FilmCraft is a pure-Rust video editor. Phase B makes it render sr-core scene documents (`.scene`) by spawning the
external `scene-render` program. Read these first:
- `AGENTS.md` and `CLAUDE.md` in your worktree. Their rules are binding.
- `docs/architecture.md`.
- The phase B plan: `/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/PLAN-B.md`.

The crate you work in is `crates/scenerender` (`filmcraft-scenerender`, layer L3). Its skeleton is committed:
`lib.rs` declares the modules and `Quality`, and each module file holds only its doc comment.

## Your environment
- Your worktree is `/home/admin/src/filmcraft-wt/runner`, on git branch `b/runner`. Other agents work in parallel on
  other modules of the same crate in their own worktrees.
- **Only edit the files in your scope** (listed below). If you need a change outside it, describe it in your reply
  instead of making it.
  - `lib.rs` is shared: add nothing to it except `pub use` lines or a `#[cfg(test)] mod …;` line for your own modules.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-runner` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh runner cargo test -p filmcraft-scenerender
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh runner sh -c 'cargo clippy -p filmcraft-scenerender --all-targets -- -D warnings && cargo test -p filmcraft-scenerender'

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

**B2: running the renderer, and reading its frames back.** Scope: `src/process.rs`, `src/ingest.rs`, and a new
test-support module `src/fake.rs` declared in `lib.rs` as `#[cfg(all(test, unix))] pub(crate) mod fake;`. Later
agents reuse it, so give it a clean API.

1. **`process`**:
   - `pub struct RunOutput { pub code: Option<i32>, pub stdout_tail: String, pub stderr_tail: String }`.
   - `pub trait Runner: Send + Sync { fn run(&self, program: &Path, args: &[OsString], cwd: &Path, timeout: Duration) -> filmcraft_media::Result<RunOutput>; }`
   - `pub struct ProcessRunner` uses `std::process::Command`:
     - null stdin; stdout and stderr piped;
     - each pipe drained by its own thread, which keeps only the last 64 KiB and converts it with lossy UTF-8, so a
       chatty child can never block or blow memory;
     - polls `try_wait` every ~10–20 ms.
   - On timeout, and when `filmcraft_media::cancel::cancelled()` turns true (read `crates/media/src/cancel.rs`; it is
     thread-scoped, and the poll loop runs on the caller's thread), it kills the child, reaps it (no zombies) and
     returns `MediaError::Io("scene-render timed out after …")` or `MediaError::Cancelled` respectively.
   - Join-thread failures map to errors; they never panic.
   - `pub fn check(out: &RunOutput) -> filmcraft_media::Result<()>`:
     - 0 → Ok;
     - 1 → `MediaError::Decode("scene-render rejected the scene: <stderr tail, trimmed, ≤ 2 KiB>")`;
     - any other code or a signal → `MediaError::Io(…)` with the tail.
   - It is native only. On wasm `ProcessRunner::run` returns `MediaError::Unsupported`.
   - Tests use `/bin/sh -c` children:
     - exit codes 0/1/2/137;
     - a large stdout (several MB) that must not hang and whose kept tail is bounded;
     - a sleep past a short timeout, checking that it is killed promptly;
     - cancellation through `filmcraft_media::cancel::with_cancel`;
     - a missing program, which must give `MediaError::Io`, not a panic.
2. **`ingest`**: `pub fn read_frame(path: &Path, width: u32, height: u32, max_bytes: u64) -> filmcraft_media::Result<filmcraft_frame::VideoFrame>`.
   - Check the file length against `max_bytes` *before* reading it.
   - Decode with `filmcraft_media::still::decode_raster`.
   - Require the decoded size to equal width×height.
   - Return straight-alpha sRGB RGBA8. Mirror how `filmcraft_media::still::StillSource` builds its frame and its
     `ColorInfo`.
   - Tests:
     - a good PNG;
     - a wrong size;
     - a truncated or garbage file;
     - an over-cap file;
     - a missing file;
     - a 16-bit PNG, which decodes to 8-bit.
3. **`fake`**: a fake `scene-render` for tests: a `#!/bin/sh` script written into a fresh temp dir (unique per test;
   no tempfile crate, so use `std::env::temp_dir()` plus pid plus a counter, removed on Drop).
   - `FakeRenderer::new(behaviour: Behaviour) -> FakeRenderer` with `.program() -> &Path` and `.invocations() -> Vec<String>`.
     Every call appends its argv line to a log file in the dir.
   - It answers:
     - `--version` → `scene-render 0.5.0`;
     - `capabilities` → a minimal valid JSON with format `scene-render-capabilities/1`;
     - `render DOC --frames A..B -o PATTERN …`. Parse the flags **by name**, not by position. For each k in A..B it
       copies a template PNG to `printf "$PATTERN" k`. The template is written by the test helper with the `image`
       crate at the size you choose (`Behaviour::Ok { width, height }`).
     - `encode DOC -o X.wav`: write a tiny valid WAV, using `filmcraft_media::wav::write_wav16`, created by the helper
       as a template file.
   - Behaviours: `Ok`, `ExitInvalid` (exit 1 with a diagnostic on stderr), `ExitIo` (exit 2), `WrongSize`,
     `NoOutput` (exit 0 but writes nothing), `Hang` (sleeps 30 s), `NoAudio` (encode exits 1 with
     "audio-only output, but the scene has no audio").
   - It is only test code, but it must not leave junk behind.
   - Use it in `process` tests: run the fake's render through `ProcessRunner` and ingest the frames.

The argv layout is fixed by the B1 `args` module, which another agent writes in parallel:
`render <doc> --frames A..B -o <pattern> --bit-depth 8 --quality <q>`, `encode <doc> -o <wav>`, `--version`,
`capabilities`. Build these argv by hand in your tests. Don't depend on `args`.
