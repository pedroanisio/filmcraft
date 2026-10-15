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
