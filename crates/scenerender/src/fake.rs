//! Unix test renderer with per-instance files and automatic cleanup.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Renderer outcome for render and encode invocations; probes always succeed.
#[derive(Clone, Copy, Debug)]
pub enum Behaviour {
    /// Write frames at the requested fixture size and a short mono WAV.
    Ok { width: u32, height: u32 },
    /// Reject the scene with exit code 1.
    ExitInvalid,
    /// Fail with exit code 2.
    ExitIo,
    /// Write a 1 × 1 PNG regardless of the scene's dimensions.
    WrongSize,
    /// Succeed without creating output files.
    NoOutput,
    /// Wait for 30 seconds.
    Hang,
    /// Render normally at 2 × 2, but reject audio-only output.
    NoAudio,
}

/// A disposable renderer executable, invocation log, and fixture directory.
pub struct FakeRenderer {
    dir: PathBuf,
    program: PathBuf,
}

impl FakeRenderer {
    /// Create an isolated fake renderer; setup failures fail the owning test.
    pub fn new(behaviour: Behaviour) -> Self {
        use std::os::unix::fs::PermissionsExt;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = loop {
            let dir = std::env::temp_dir().join(format!("filmcraft-renderer-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("create fake renderer: {e}"),
            }
        };
        let fake = Self { program: dir.join("scene-render"), dir };
        let (mode, width, height) = match behaviour {
            Behaviour::Ok { width, height } => ("ok", width, height),
            Behaviour::ExitInvalid => ("invalid", 2, 2),
            Behaviour::ExitIo => ("io", 2, 2),
            Behaviour::WrongSize => ("ok", 1, 1),
            Behaviour::NoOutput => ("empty", 2, 2),
            Behaviour::Hang => ("hang", 2, 2),
            Behaviour::NoAudio => ("no-audio", 2, 2),
        };
        assert!(width > 0 && height > 0 && u64::from(width) * u64::from(height) <= 16 * 1024 * 1024, "fake fixture dimensions are too large");
        image::RgbaImage::from_pixel(width, height, image::Rgba([200, 100, 50, 128])).save(fake.dir.join("template.png")).unwrap();
        std::fs::write(fake.dir.join("template.wav"), filmcraft_media::wav::write_wav16(&[0.0; 48], 1, 48_000)).unwrap();
        std::fs::write(&fake.program, SCRIPT.replace("@MODE@", mode)).unwrap();
        std::fs::set_permissions(&fake.program, std::fs::Permissions::from_mode(0o700)).unwrap();
        fake
    }

    /// Absolute path of the executable script.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Directory available to the owning test for documents and output files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Recorded argv lines, in invocation order (arguments separated by spaces).
    pub fn invocations(&self) -> Vec<String> {
        match std::fs::read_to_string(self.dir.join("invocations.log")) {
            Ok(s) => s.lines().map(str::to_owned).collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("read fake renderer invocations: {e}"),
        }
    }
}

impl Drop for FakeRenderer {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.dir) {
            eprintln!("remove fake renderer {}: {e}", self.dir.display());
        }
    }
}

const SCRIPT: &str = r#"#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$root/invocations.log"
case "${1-}" in
    --version) printf 'scene-render 0.5.0\n'; exit 0 ;;
    capabilities) printf '%s\n' '{"format":"scene-render-capabilities/1","engine":{"name":"rs-scene-render","version":"0.5.0"},"schema":"1.7.0","entries":[]}'; exit 0 ;;
esac
mode='@MODE@'
case "$mode" in
    invalid) printf 'invalid scene\n' >&2; exit 1 ;;
    io) printf 'I/O failure\n' >&2; exit 2 ;;
    empty) exit 0 ;;
    hang) exec sleep 30 ;;
esac
command=${1-}
shift
if [ "$command" = encode ] && [ "$mode" = no-audio ]; then
    printf 'audio-only output, but the scene has no audio\n' >&2
    exit 1
fi
output=''
frames=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o|--output) output=$2; shift 2 ;;
        --frames) frames=$2; shift 2 ;;
        --bit-depth|--quality) shift 2 ;;
        *) shift ;;
    esac
done
[ -n "$output" ] || exit 2
case "$command" in
    render)
        start=${frames%%..*}
        end=${frames##*..}
        case "$start:$end" in *[!0-9:]*|:*|*:) exit 2 ;; esac
        [ "$end" -ge "$start" ] && [ "$end" -le 1000000 ] && [ "$((end-start))" -le 10000 ] || exit 2
        k=$start
        while [ "$k" -lt "$end" ]; do
            path=$(printf "$output" "$k")
            cp "$root/template.png" "$path"
            k=$((k+1))
        done ;;
    encode) cp "$root/template.wav" "$output" ;;
    *) exit 2 ;;
esac
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Output},
        time::{Duration, Instant},
    };

    fn launch<T>(mut operation: impl FnMut() -> std::io::Result<T>) -> T {
        let start = Instant::now();
        loop {
            match operation() {
                Ok(value) => return value,
                Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && start.elapsed() < Duration::from_secs(2) => {
                    std::thread::sleep(Duration::from_millis(15));
                }
                Err(e) => panic!("launch fake renderer: {e}"),
            }
        }
    }

    fn invoke(fake: &FakeRenderer, args: &[&str]) -> Output {
        launch(|| Command::new(fake.program()).args(args).current_dir(fake.dir()).output())
    }

    #[test]
    fn probes_and_invocations() {
        let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 3 });
        let out = invoke(&fake, &["--version"]);
        assert!(out.status.success());
        assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), "scene-render 0.5.0");
        let out = invoke(&fake, &["capabilities"]);
        assert!(out.status.success());
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(json["format"], "scene-render-capabilities/1");
        assert_eq!(json["engine"]["version"], "0.5.0");
        assert!(json["schema"].is_string());
        assert_eq!(fake.invocations(), ["--version", "capabilities"]);
    }

    #[test]
    fn render_flags_are_named_and_range_is_exclusive() {
        let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 3 });
        let out = invoke(&fake, &["render", "doc with spaces.scene", "--quality", "final", "-o", "out %06d.png", "--bit-depth", "8", "--frames", "7..9"]);
        assert!(out.status.success());
        for k in 7..9 {
            let image = image::open(fake.dir().join(format!("out {k:06}.png"))).unwrap();
            assert_eq!((image.width(), image.height()), (2, 3));
        }
        assert!(!fake.dir().join("out 000009.png").exists());
    }

    #[test]
    fn encode_writes_valid_wav() {
        let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 3 });
        let out = invoke(&fake, &["encode", "doc.scene", "-o", "mix.wav"]);
        assert!(out.status.success());
        let bytes = std::fs::read(fake.dir().join("mix.wav")).unwrap();
        assert_eq!(bytes, filmcraft_media::wav::write_wav16(&[0.0; 48], 1, 48_000));
    }

    #[test]
    fn failure_and_output_behaviours() {
        for (behaviour, code, diagnostic) in [(Behaviour::ExitInvalid, 1, "invalid scene"), (Behaviour::ExitIo, 2, "I/O failure"), (Behaviour::NoOutput, 0, "")]
        {
            let fake = FakeRenderer::new(behaviour);
            let out = invoke(&fake, &["render", "doc.scene", "--frames", "0..1", "-o", "%06d.png"]);
            assert_eq!(out.status.code(), Some(code));
            assert!(String::from_utf8_lossy(&out.stderr).contains(diagnostic));
            assert!(!fake.dir().join("000000.png").exists());
        }
        let fake = FakeRenderer::new(Behaviour::WrongSize);
        assert!(invoke(&fake, &["render", "doc.scene", "--frames", "0..1", "-o", "%06d.png"]).status.success());
        let image = image::open(fake.dir().join("000000.png")).unwrap();
        assert_eq!((image.width(), image.height()), (1, 1));
    }

    #[test]
    fn no_audio_keeps_video_available() {
        let fake = FakeRenderer::new(Behaviour::NoAudio);
        let out = invoke(&fake, &["encode", "doc.scene", "-o", "mix.wav"]);
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains("audio-only output, but the scene has no audio"));
        assert!(invoke(&fake, &["render", "doc.scene", "--frames", "0..1", "-o", "%06d.png"]).status.success());
        assert!(fake.dir().join("000000.png").exists());
    }

    #[test]
    fn hang_is_killable_and_directories_are_unique_and_removed() {
        let first = FakeRenderer::new(Behaviour::Hang);
        let second = FakeRenderer::new(Behaviour::NoOutput);
        assert_ne!(first.dir(), second.dir());
        let path = first.dir().to_owned();
        let mut child = launch(|| Command::new(first.program()).args(["render", "doc.scene"]).spawn());
        std::thread::sleep(Duration::from_millis(50));
        let still_running = child.try_wait().unwrap().is_none();
        let start = Instant::now();
        let _ = child.kill();
        child.wait().unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(first);
        assert!(!path.exists());
        assert!(still_running);
    }
}
