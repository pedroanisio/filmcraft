//! Shell-free argument vectors for the scene-render CLI.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use crate::Quality;
use filmcraft_media::{MediaError, Result};
use std::{ffi::OsString, ops::Range, path::Path};

fn check_path(path: &Path, label: &str) -> Result<()> {
    if !path.is_absolute() {
        return Err(MediaError::Unsupported(format!("scene-render {label} must be an absolute path")));
    }
    if path.as_os_str().len() > 64 << 10 {
        return Err(MediaError::Unsupported(format!("scene-render {label} exceeds 64 KiB")));
    }
    Ok(())
}

/// Render an end-exclusive, nonempty frame range to an absolute PNG sequence pattern.
/// Both paths are capped at 64 KiB and passed verbatim as OS strings.
pub fn render_args(doc: &Path, frames: Range<u64>, pattern: &Path, quality: Quality) -> Result<Vec<OsString>> {
    check_path(doc, "document")?;
    check_path(pattern, "output")?;
    if frames.start >= frames.end {
        return Err(MediaError::Unsupported("scene-render frame range must be nonempty and increasing".into()));
    }
    Ok(vec![
        "render".into(),
        doc.as_os_str().into(),
        "--frames".into(),
        format!("{}..{}", frames.start, frames.end).into(),
        "-o".into(),
        pattern.as_os_str().into(),
        "--bit-depth".into(),
        "8".into(),
        "--quality".into(),
        quality.as_str().into(),
    ])
}

/// Encode the scene's mix to an absolute WAV path using the native audio writer.
/// Both paths are capped at 64 KiB. The renderer requires its output path to be UTF-8.
pub fn audio_args(doc: &Path, wav: &Path) -> Result<Vec<OsString>> {
    check_path(doc, "document")?;
    check_path(wav, "audio output")?;
    if wav.to_str().is_none() {
        return Err(MediaError::Unsupported("scene-render encode requires a UTF-8 output path".into()));
    }
    if !wav.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("wav")) {
        return Err(MediaError::Unsupported("scene-render audio output must have a .wav extension".into()));
    }
    Ok(vec!["encode".into(), doc.as_os_str().into(), "-o".into(), wav.as_os_str().into()])
}

/// Query the renderer version.
pub fn version_args() -> Vec<OsString> {
    vec!["--version".into()]
}

/// Query the renderer's machine-readable capability manifest.
pub fn capabilities_args() -> Vec<OsString> {
    vec!["capabilities".into()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_media::MediaError;
    use std::path::PathBuf;

    fn absolute(name: &str) -> PathBuf {
        if cfg!(windows) { Path::new("C:\\").join(name) } else { Path::new("/").join(name) }
    }

    #[test]
    fn render_vector_matches_cli_for_every_quality() {
        let doc = absolute("scene with spaces;$(echo nope).scene");
        let pattern = absolute("frames/%06d.png");
        for quality in [Quality::Draft, Quality::Preview, Quality::Final] {
            let expected: Vec<OsString> = vec![
                "render".into(),
                doc.as_os_str().into(),
                "--frames".into(),
                "4..19".into(),
                "-o".into(),
                pattern.as_os_str().into(),
                "--bit-depth".into(),
                "8".into(),
                "--quality".into(),
                quality.as_str().into(),
            ];
            assert_eq!(render_args(&doc, 4..19, &pattern, quality).unwrap(), expected);
        }
        assert!(render_args(&doc, (u64::MAX - 1)..u64::MAX, &pattern, Quality::Final).is_ok());
    }

    #[test]
    fn rejects_relative_paths_and_empty_ranges() {
        let doc = absolute("in.scene");
        let out = absolute("%06d.png");
        for (d, range, p) in [
            (Path::new("--version"), 0..1, out.as_path()),
            (doc.as_path(), 0..1, Path::new("out.png")),
            (doc.as_path(), 4..4, out.as_path()),
            (doc.as_path(), Range { start: 5, end: 4 }, out.as_path()),
        ] {
            assert!(matches!(render_args(d, range, p, Quality::Preview), Err(MediaError::Unsupported(_))));
        }
    }

    #[test]
    fn audio_vector_uses_wav_extension_without_extra_flags() {
        let doc = absolute("in.scene");
        let wav = absolute("mix.WAV");
        assert_eq!(audio_args(&doc, &wav).unwrap(), vec![OsString::from("encode"), doc.into_os_string(), OsString::from("-o"), wav.into_os_string()]);
    }

    #[test]
    fn audio_rejects_relative_paths_and_non_wav_outputs() {
        let doc = absolute("in.scene");
        let wav = absolute("mix.wav");
        assert!(audio_args(Path::new("in.scene"), &wav).is_err());
        assert!(audio_args(&doc, Path::new("mix.wav")).is_err());
        assert!(audio_args(&doc, &absolute("mix.mp4")).is_err());
        assert!(audio_args(&doc, &absolute("mix")).is_err());
    }

    #[test]
    fn probe_vectors_match_cli() {
        assert_eq!(version_args(), vec![OsString::from("--version")]);
        assert_eq!(capabilities_args(), vec![OsString::from("capabilities")]);
    }

    #[cfg(unix)]
    #[test]
    fn render_preserves_non_utf8_paths_but_encode_requires_utf8_output() {
        use std::os::unix::ffi::OsStringExt;
        let doc = PathBuf::from(OsString::from_vec(b"/doc-\xff.scene".to_vec()));
        let out = PathBuf::from(OsString::from_vec(b"/out-\xff.wav".to_vec()));
        let args = render_args(&doc, 0..1, &out, Quality::Draft).unwrap();
        assert_eq!(args.get(1), Some(&doc.into_os_string()));
        assert!(audio_args(&absolute("in.scene"), &out).is_err());
    }
}
