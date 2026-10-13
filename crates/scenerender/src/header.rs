//! Bounded scene facts read without invoking the renderer.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use filmcraft_interchange::scene::{
    read::{Limits, decode, read},
    values,
};
use filmcraft_media::{MediaError, Result};
use filmcraft_time::{FrameRate, Tick};
use std::collections::HashSet;

/// Scene facts needed by the media bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneHeader {
    /// Frame width in pixels (at most 16384).
    pub width: u32,
    /// Frame height in pixels (at most 16384).
    pub height: u32,
    /// Rational frame rate.
    pub fps: FrameRate,
    /// Project duration on FilmCraft's tick clock.
    pub duration: Tick,
    /// Conservative indication that assets or elements can produce sound.
    pub has_audio: bool,
    /// File reference values in document order, deduplicated (4096 entries, 4096 bytes each).
    pub asset_refs: Vec<String>,
}

impl SceneHeader {
    /// Number of frames using the renderer's one-nanosecond boundary tolerance.
    pub fn frame_count(&self) -> u64 {
        super::timing::frame_count(self.duration, self.fps)
    }
}

const MAX_SIDE: i64 = 16_384;
const MAX_ASSET_REFS: usize = 4096;
const MAX_ASSET_REF_BYTES: usize = 4096;

/// Read a bounded sr-core document and its required project attributes.
/// Dimensions are 1..=16384; references are capped at 4096 distinct values of 4096 bytes each.
/// This reads metadata, not full schema validity; other constructs are left to the renderer.
pub fn parse(bytes: &[u8]) -> Result<SceneHeader> {
    let limits = Limits::default();
    let error = |e| MediaError::Decode(format!("scene header: {e}"));
    let text = decode(bytes, &limits).map_err(|e| error(e.to_string()))?;
    let doc = read(text, &limits).map_err(|e| error(e.to_string()))?;
    let mut projects = doc.root().children().filter(|n| n.has_tag_name("project"));
    let project = projects.next().ok_or_else(|| error("missing <project>".into()))?;
    if projects.next().is_some() {
        return Err(error("more than one <project>".into()));
    }
    let attribute = |name| -> Result<&str> {
        let value = project.attribute(name).ok_or_else(|| error(format!("<project> is missing {name}")))?;
        if value.len() > 128 {
            return Err(error(format!("project@{name} exceeds 128 bytes")));
        }
        Ok(value)
    };
    let size = |name| -> Result<u32> {
        let n = values::integer_in(attribute(name)?, 1, MAX_SIDE).map_err(|e| error(format!("project@{name}: {e}")))?;
        u32::try_from(n).map_err(|e| error(format!("project@{name}: {e}")))
    };
    let width = size("width")?;
    let height = size("height")?;
    let fps = values::fps(attribute("fps")?).map_err(|e| error(format!("project@fps: {e}")))?;
    let seconds = values::duration(attribute("duration")?).map_err(|e| error(format!("project@duration: {e}")))?;
    let duration = values::seconds_to_tick(seconds).ok_or_else(|| error("project@duration is outside the tick range".into()))?;
    let mut has_audio = false;
    let mut asset_refs = Vec::new();
    let mut seen = HashSet::new();
    for node in doc.doc.descendants().filter(|n| n.is_element()) {
        has_audio |= matches!(node.tag_name().name(), "audio" | "video" | "audioTrack")
            || (node.has_tag_name("generated") && matches!(node.attribute("kind"), Some("speech" | "music" | "sound-effect" | "video")));
        for attr in node.attributes().filter(|a| file_attribute(node.tag_name().name(), a.name())) {
            let value = attr.value();
            if value.len() > MAX_ASSET_REF_BYTES {
                return Err(error(format!("{}@{} file reference exceeds {MAX_ASSET_REF_BYTES} bytes", node.tag_name().name(), attr.name())));
            }
            if !seen.contains(value) {
                if asset_refs.len() >= MAX_ASSET_REFS {
                    return Err(error(format!("more than {MAX_ASSET_REFS} distinct file references")));
                }
                seen.insert(value);
                asset_refs.push(value.to_owned());
            }
        }
    }
    Ok(SceneHeader { width, height, fps, duration, has_audio, asset_refs })
}

fn file_attribute(element: &str, name: &str) -> bool {
    if name == "path" {
        // Other path attributes contain SVG geometry, not file names.
        return matches!(element, "output" | "poster" | "thumbnail");
    }
    // URI attributes in sr-core 1.7, plus sequence patterns and href extensions.
    matches!(
        name,
        "src"
            | "href"
            | "proxy"
            | "fontFile"
            | "cache"
            | "baseColorMap"
            | "normalMap"
            | "metallicRoughnessMap"
            | "occlusionMap"
            | "emissiveMap"
            | "displacementMap"
            | "materialX"
            | "weights"
            | "shader"
            | "ies"
            | "environment"
            | "ocioConfig"
            | "uri"
            | "report"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_media::MediaError;
    use filmcraft_time::TICKS_PER_SECOND;

    fn scene(fps: &str, duration: &str, body: &str) -> String {
        format!(r#"<scene version="1.7"><project width="1920" height="1080" fps="{fps}" duration="{duration}"/>{body}<composition/></scene>"#)
    }

    #[test]
    fn reads_project_facts_and_bom() {
        let xml = scene("30000/1001", "1.001", "");
        let header = parse(format!("\u{feff}{xml}").as_bytes()).unwrap();
        assert_eq!((header.width, header.height), (1920, 1080));
        assert_eq!(header.fps, FrameRate::FPS_29_97);
        assert_eq!(header.duration, Tick(254_270_016_000));
        assert_eq!(header.frame_count(), 30);
        assert!(!header.has_audio);
        assert!(header.asset_refs.is_empty());
    }

    #[test]
    fn missing_duplicate_or_nested_project_is_decode_error() {
        for bad in [
            r#"<scene version="1.7"/>"#.to_owned(),
            r#"<scene version="1.7"><group><project width="1" height="1" fps="1" duration="1"/></group></scene>"#.to_owned(),
            scene("24", "1", r#"<project width="1" height="1" fps="1" duration="1"/>"#),
        ] {
            assert!(matches!(parse(bad.as_bytes()), Err(MediaError::Decode(message)) if message.contains("project")));
        }
    }

    #[test]
    fn rejects_hostile_project_values_with_context() {
        let valid = scene("24", "1", "");
        for (from, to, field) in [
            ("1920", "999999999999999999999999", "width"),
            ("1920", "16385", "width"),
            ("1920", "0", "width"),
            ("1080", "0", "height"),
            ("fps=\"24\"", "fps=\"0/1\"", "fps"),
            ("duration=\"1\"", "duration=\"NaN\"", "duration"),
            ("duration=\"1\"", "duration=\"0\"", "duration"),
            ("duration=\"1\"", "duration=\"1000001\"", "duration"),
            ("width=\"1920\"", "", "width"),
        ] {
            let bad = valid.replace(from, to);
            assert!(matches!(parse(bad.as_bytes()), Err(MediaError::Decode(message)) if message.contains(field)), "{bad}");
        }
        assert!(parse(valid.replace("1920", "16384").as_bytes()).is_ok());
    }

    #[test]
    fn audio_detection_includes_unused_video_and_generated_sound() {
        for body in [
            r#"<assets><audio src="a.wav"/></assets>"#,
            r#"<assets><video src="v.mp4" hasAudio="false"/></assets>"#,
            r#"<assets><generated kind="speech" cache="a.wav"/></assets>"#,
            r#"<assets><generated kind="music" cache="a.wav"/></assets>"#,
            r#"<assets><generated kind="sound-effect" cache="a.wav"/></assets>"#,
            r#"<assets><generated kind="video" cache="v.mp4"/></assets>"#,
            r#"<audioMix><audioTrack asset="sound"/></audioMix>"#,
        ] {
            assert!(parse(scene("24", "1", body).as_bytes()).unwrap().has_audio, "{body}");
        }
        assert!(!parse(scene("24", "1", r#"<assets><image src="a.png"/><generated kind="image" cache="b.png"/></assets>"#).as_bytes()).unwrap().has_audio);
    }

    #[test]
    fn file_references_are_complete_ordered_and_deduplicated() {
        let body = r#"<assets><image src="a.png" proxy="proxy.png"/><text fontFile="font.ttf"/><generated kind="image" cache="cache.png"/><vector href="a.png"/></assets><material baseColorMap="base.png" normalMap="normal.png" metallicRoughnessMap="mr.png" occlusionMap="ao.png" emissiveMap="em.png" displacementMap="height.png" materialX="look.mtlx"/><deformer weights="weights.bin"/><transition shader="effect.wgsl"/><light ies="light.ies" environment="env.hdr"/><colorManagement ocioConfig="config.ocio"/><poster path="poster.png"/><source uri="source.png"/><output report="report.json"/><image src="a.png"/>"#;
        assert_eq!(
            parse(scene("24", "1", body).as_bytes()).unwrap().asset_refs,
            [
                "a.png",
                "proxy.png",
                "font.ttf",
                "cache.png",
                "base.png",
                "normal.png",
                "mr.png",
                "ao.png",
                "em.png",
                "height.png",
                "look.mtlx",
                "weights.bin",
                "effect.wgsl",
                "light.ies",
                "env.hdr",
                "config.ocio",
                "poster.png",
                "source.png",
                "report.json"
            ]
        );
    }

    #[test]
    fn geometry_paths_are_not_file_references() {
        let body = r#"<assets><vector shape="path" path="M0 0 L1 1"/></assets><motionPath path="M2 2 L3 3"/><output path="movie.mp4"><poster path="poster.jpg"/><thumbnail path="thumb.jpg"/></output>"#;
        assert_eq!(parse(scene("24", "1", body).as_bytes()).unwrap().asset_refs, ["movie.mp4", "poster.jpg", "thumb.jpg"]);
        let long = format!(r#"<vector shape="path" path="M0 0 {}"/>"#, "L1 1 ".repeat(1000));
        assert!(parse(scene("24", "1", &long).as_bytes()).unwrap().asset_refs.is_empty());
    }

    #[test]
    fn file_references_have_length_and_count_caps() {
        let long = format!(r#"<image src="{}"/>"#, "x".repeat(4097));
        assert!(parse(scene("24", "1", &long).as_bytes()).is_err());
        let many = (0..4097).map(|i| format!(r#"<image src="{i}.png"/>"#)).collect::<String>();
        assert!(parse(scene("24", "1", &many).as_bytes()).is_err());
        let repeats = r#"<image src="same.png"/>"#.repeat(4097);
        assert_eq!(parse(scene("24", "1", &repeats).as_bytes()).unwrap().asset_refs, ["same.png"]);
    }

    #[test]
    fn seconds_conversion_keeps_renderer_counts_at_broadcast_rates() {
        for (text, fps) in [("24", FrameRate::FPS_24), ("25", FrameRate::FPS_25), ("30000/1001", FrameRate::FPS_29_97), ("60", FrameRate::FPS_60)] {
            for k in [1, 2, 37, 9999, 20_000_001] {
                let seconds = k as f64 * fps.den as f64 / fps.num as f64;
                for extra in [0.0, 0.000001, 0.25 * fps.den as f64 / fps.num as f64] {
                    let duration = seconds + extra;
                    let f = duration * fps.num as f64 / fps.den as f64;
                    let expected = if (f - f.round()).abs() <= 1e-9 * fps.num as f64 / fps.den as f64 { f.round() as u64 } else { f.ceil() as u64 };
                    let header = parse(scene(text, &duration.to_string(), "").as_bytes()).unwrap();
                    assert_eq!(header.frame_count(), expected, "fps={text}, duration={duration}");
                }
            }
        }
        assert_eq!(parse(scene("25", "1", "").as_bytes()).unwrap().duration, Tick(TICKS_PER_SECOND));
    }

    #[test]
    fn xml_mutations_and_hostile_structure_never_panic() {
        let xml = scene("24", "1", "");
        for i in 0..xml.len() {
            assert!(std::panic::catch_unwind(|| parse(&xml.as_bytes()[..i])).unwrap().is_err());
            let mut bytes = xml.as_bytes().to_vec();
            bytes[i] ^= 0x80;
            assert!(std::panic::catch_unwind(|| parse(&bytes)).unwrap().is_err());
        }
        let deep = format!(r#"<scene version="1.7">{}{}</scene>"#, "<g>".repeat(300), "</g>".repeat(300));
        assert!(parse(deep.as_bytes()).is_err());
        assert!(parse(b"\xff\xfe").is_err());
        assert!(parse(b"<!DOCTYPE scene [<!ENTITY a 'bad'>]><scene version='1.7'>&a;</scene>").is_err());
    }
}
