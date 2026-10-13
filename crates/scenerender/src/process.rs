//! Running the renderer as a child process: bounded output, deadline and cancellation, exit-code mapping.

use std::{ffi::OsString, path::Path, time::Duration};

use filmcraft_media::{MediaError, Result};

/// Exit status and bounded, lossy UTF-8 diagnostic tails from one invocation.
#[derive(Debug)]
pub struct RunOutput {
    /// Exit code, or `None` when terminated by a signal.
    pub code: Option<i32>,
    /// Last 64 KiB of standard output.
    pub stdout_tail: String,
    /// Last 64 KiB of standard error.
    pub stderr_tail: String,
}

/// Executes the external renderer; callers can substitute a runner in tests.
pub trait Runner: Send + Sync {
    /// Run with a wall-clock timeout and the calling thread's media cancellation flag.
    fn run(&self, program: &Path, args: &[OsString], cwd: &Path, timeout: Duration) -> Result<RunOutput>;
}

/// Native child-process runner, unavailable on wasm.
#[derive(Debug, Default)]
pub struct ProcessRunner;

impl Runner for ProcessRunner {
    #[cfg(not(target_arch = "wasm32"))]
    fn run(&self, program: &Path, args: &[OsString], cwd: &Path, timeout: Duration) -> Result<RunOutput> {
        native::run(program, args, cwd, timeout)
    }

    #[cfg(target_arch = "wasm32")]
    fn run(&self, _program: &Path, _args: &[OsString], _cwd: &Path, _timeout: Duration) -> Result<RunOutput> {
        Err(MediaError::Unsupported("scene-render processes are unavailable on wasm".into()))
    }
}

/// Translate the renderer's exit status into a media error with a bounded diagnostic.
pub fn check(out: &RunOutput) -> Result<()> {
    let diagnostic = text_tail(out.stderr_tail.trim(), 2048);
    match out.code {
        Some(0) => Ok(()),
        Some(1) => Err(MediaError::Decode(format!("scene-render rejected the scene: {diagnostic}"))),
        Some(code) => Err(MediaError::Io(format!("scene-render exited with code {code}: {diagnostic}"))),
        None => Err(MediaError::Io(format!("scene-render terminated by a signal: {diagnostic}"))),
    }
}

fn text_tail(text: &str, limit: usize) -> &str {
    let mut start = text.len().saturating_sub(limit);
    while !text.is_char_boundary(start) {
        start = start.saturating_add(1);
    }
    text.get(start..).unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::{
        collections::VecDeque,
        io::{self, Read},
        panic::{AssertUnwindSafe, catch_unwind},
        process::{Child, Command, Stdio},
        thread::{self, JoinHandle},
        time::Instant,
    };

    const TAIL_BYTES: usize = 64 * 1024;
    const POLL: Duration = Duration::from_millis(15);

    struct ChildGuard(Child);

    impl ChildGuard {
        fn stop(&mut self) -> Result<()> {
            if self.0.try_wait().map_err(|e| io_error("poll during cleanup", e))?.is_some() {
                return Ok(());
            }
            let killed = self.0.kill();
            let reaped = self.0.wait();
            reaped.map_err(|e| io_error("reap", e))?;
            killed.map_err(|e| io_error("kill", e))
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            // Also clean up if thread creation or an unexpected caller unwind interrupts setup.
            let _ = self.stop();
        }
    }

    fn io_error(action: &str, error: impl std::fmt::Display) -> MediaError {
        MediaError::Io(format!("scene-render {action}: {error}"))
    }

    fn drain(mut pipe: impl Read) -> Result<String> {
        let mut tail = VecDeque::with_capacity(TAIL_BYTES);
        let mut buffer = [0_u8; 8192];
        loop {
            let count = match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => count,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_error("read output", e)),
            };
            let chunk = buffer.get(..count).ok_or_else(|| io_error("read output", "invalid byte count"))?;
            let excess = tail.len().saturating_add(count).saturating_sub(TAIL_BYTES);
            tail.drain(..excess.min(tail.len()));
            tail.extend(chunk);
        }
        let text = String::from_utf8_lossy(tail.make_contiguous());
        // Replacement characters can expand an invalid UTF-8 byte into three bytes.
        Ok(text_tail(&text, TAIL_BYTES).to_owned())
    }

    fn reader(pipe: impl Read + Send + 'static, name: &str) -> Result<JoinHandle<Result<String>>> {
        thread::Builder::new()
            .name(name.into())
            .spawn(move || catch_unwind(AssertUnwindSafe(|| drain(pipe))).map_err(|_| io_error("read output", "worker panicked"))?)
            .map_err(|e| io_error("start output reader", e))
    }

    fn join(handle: JoinHandle<Result<String>>) -> Result<String> {
        handle.join().map_err(|_| io_error("join output reader", "worker panicked"))?
    }

    pub(super) fn run(program: &Path, args: &[OsString], cwd: &Path, timeout: Duration) -> Result<RunOutput> {
        if filmcraft_media::cancel::cancelled() {
            return Err(MediaError::Cancelled);
        }
        let start = Instant::now();
        let mut command = Command::new(program);
        command.args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = loop {
            if filmcraft_media::cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            if start.elapsed() >= timeout {
                return Err(MediaError::Io(format!("scene-render timed out after {timeout:?}")));
            }
            match command.spawn() {
                Ok(child) => break child,
                // An installer, or an inherited writer during a concurrent fork, may still hold the executable open.
                Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy => thread::sleep(POLL.min(timeout.saturating_sub(start.elapsed()))),
                Err(e) => return Err(io_error("spawn", e)),
            }
        };
        let mut child = ChildGuard(child);
        let stdout = child.0.stdout.take().ok_or_else(|| io_error("capture stdout", "pipe unavailable"))?;
        let stderr = child.0.stderr.take().ok_or_else(|| io_error("capture stderr", "pipe unavailable"))?;
        let stdout = reader(stdout, "scene-render-stdout")?;
        let stderr = reader(stderr, "scene-render-stderr")?;
        let result = loop {
            if filmcraft_media::cancel::cancelled() {
                break Err(MediaError::Cancelled);
            }
            match child.0.try_wait() {
                Ok(Some(status)) if stdout.is_finished() && stderr.is_finished() => break Ok(status.code()),
                Ok(_) => {}
                Err(e) => break Err(io_error("poll", e)),
            }
            if start.elapsed() >= timeout {
                break Err(MediaError::Io(format!("scene-render timed out after {timeout:?}")));
            }
            thread::sleep(POLL.min(timeout.saturating_sub(start.elapsed())));
        };
        if result.is_err() {
            child.stop()?;
        }
        // A descendant can inherit a pipe. Never let joining that reader defeat the deadline.
        // Such a reader exits at EOF and retains only its fixed-size tail in the meantime.
        let stdout = if stdout.is_finished() { join(stdout) } else { Ok(String::new()) };
        let stderr = if stderr.is_finished() { join(stderr) } else { Ok(String::new()) };
        let code = result?;
        Ok(RunOutput { code, stdout_tail: stdout?, stderr_tail: stderr? })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::{
        fake::{Behaviour, FakeRenderer},
        ingest::read_frame,
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Instant,
    };

    fn shell(script: &str, timeout: Duration) -> Result<RunOutput> {
        ProcessRunner.run(Path::new("/bin/sh"), &["-c".into(), script.into()], Path::new("/"), timeout)
    }

    #[test]
    fn exit_codes_and_diagnostics() {
        for code in [0, 1, 2, 137] {
            let out = shell(&format!("printf '  diagnostic  \\n' >&2; exit {code}"), Duration::from_secs(5)).unwrap();
            assert_eq!(out.code, Some(code));
            match (code, check(&out)) {
                (0, Ok(())) => {}
                (1, Err(MediaError::Decode(s))) => assert_eq!(s, "scene-render rejected the scene: diagnostic"),
                (2 | 137, Err(MediaError::Io(s))) => {
                    assert!(s.contains("diagnostic"));
                    assert!(s.contains(&code.to_string()));
                }
                other => panic!("unexpected exit mapping: {other:?}"),
            }
        }
    }

    #[test]
    fn diagnostic_tail_is_utf8_safe_and_bounded() {
        for code in [Some(1), Some(2), None] {
            let out = RunOutput { code, stdout_tail: String::new(), stderr_tail: format!("{}END  \n", "界".repeat(3000)) };
            let error = check(&out).unwrap_err();
            let s = match error {
                MediaError::Decode(s) | MediaError::Io(s) => s,
                other => panic!("{other:?}"),
            };
            let tail = s.rsplit_once(": ").unwrap().1;
            assert!(tail.len() <= 2048);
            assert!(tail.ends_with("END"));
        }
    }

    #[test]
    fn signal_is_an_io_error() {
        let out = shell("kill -TERM $$", Duration::from_secs(5)).unwrap();
        assert_eq!(out.code, None);
        assert!(matches!(check(&out), Err(MediaError::Io(s)) if s.contains("signal")));
    }

    #[test]
    fn drains_megabytes_from_both_pipes_and_bounds_lossy_tails() {
        let out = shell("head -c 3145728 /dev/zero; printf 'STDOUT-END'; head -c 3145728 /dev/zero >&2; printf '\\377STDERR-END' >&2", Duration::from_secs(10))
            .unwrap();
        check(&out).unwrap();
        assert!(out.stdout_tail.len() <= 65536);
        assert!(out.stderr_tail.len() <= 65536);
        assert!(out.stdout_tail.ends_with("STDOUT-END"));
        assert!(out.stderr_tail.ends_with("�STDERR-END"));
        assert!(out.stdout_tail.len() > 60000);
    }

    #[test]
    fn stdin_is_closed() {
        let out = shell("if read value; then exit 2; fi", Duration::from_secs(2)).unwrap();
        check(&out).unwrap();
    }

    #[test]
    fn timeout_kills_and_reaps_promptly() {
        let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 3 });
        let pid_file = fake.dir().join("pid");
        let start = Instant::now();
        let result = ProcessRunner.run(Path::new("/bin/sh"), &["-c".into(), "echo $$ > pid; exec sleep 30".into()], fake.dir(), Duration::from_millis(150));
        assert!(matches!(result, Err(MediaError::Io(s)) if s.starts_with("scene-render timed out after")));
        assert!(start.elapsed() < Duration::from_secs(3));
        let pid = std::fs::read_to_string(pid_file).unwrap();
        let out = shell(&format!("kill -0 {} 2>/dev/null", pid.trim()), Duration::from_secs(2)).unwrap();
        assert_ne!(out.code, Some(0), "child must be reaped");
    }

    #[test]
    fn cancellation_is_polled_on_the_caller_thread() {
        let flag = Arc::new(AtomicBool::new(false));
        let setter_flag = flag.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            setter_flag.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        let result = filmcraft_media::cancel::with_cancel(&flag, || shell("exec sleep 30", Duration::from_secs(10)));
        setter.join().unwrap();
        assert!(matches!(result, Err(MediaError::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn missing_program_is_io() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let result = ProcessRunner.run(&fake.dir().join("missing"), &[], fake.dir(), Duration::from_secs(1));
        assert!(matches!(result, Err(MediaError::Io(_))));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn busy_executable_is_retried_until_writer_closes() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let writer = std::fs::OpenOptions::new().write(true).open(fake.program()).unwrap();
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(writer);
        });
        let result = ProcessRunner.run(fake.program(), &["--version".into()], fake.dir(), Duration::from_secs(2));
        closer.join().unwrap();
        let out = result.unwrap();
        check(&out).unwrap();
        assert_eq!(out.stdout_tail.trim(), "scene-render 0.5.0");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn busy_executable_respects_timeout() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let _writer = std::fs::OpenOptions::new().write(true).open(fake.program()).unwrap();
        let start = Instant::now();
        let result = ProcessRunner.run(fake.program(), &["--version".into()], fake.dir(), Duration::from_millis(60));
        assert!(matches!(result, Err(MediaError::Io(s)) if s.starts_with("scene-render timed out after")));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn busy_executable_respects_cancellation() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let _writer = std::fs::OpenOptions::new().write(true).open(fake.program()).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        let setter_flag = flag.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            setter_flag.store(true, Ordering::Relaxed);
        });
        let result =
            filmcraft_media::cancel::with_cancel(&flag, || ProcessRunner.run(fake.program(), &["--version".into()], fake.dir(), Duration::from_secs(2)));
        setter.join().unwrap();
        assert!(matches!(result, Err(MediaError::Cancelled)));
    }

    #[test]
    fn fake_render_frames_are_ingested() {
        let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 3 });
        let pattern = fake.dir().join("frame %06d.png");
        let args = vec![
            "render".into(),
            "document with spaces.scene".into(),
            "--frames".into(),
            "4..6".into(),
            "-o".into(),
            pattern.into_os_string(),
            "--bit-depth".into(),
            "8".into(),
            "--quality".into(),
            "preview".into(),
        ];
        let out = ProcessRunner.run(fake.program(), &args, fake.dir(), Duration::from_secs(5)).unwrap();
        check(&out).unwrap();
        for k in 4..6 {
            let frame = read_frame(&fake.dir().join(format!("frame {k:06}.png")), 2, 3, 4096).unwrap();
            assert_eq!((frame.width, frame.height), (2, 3));
        }
        assert!(!fake.dir().join("frame 000006.png").exists());
        assert_eq!(fake.invocations().len(), 1);
    }
}
