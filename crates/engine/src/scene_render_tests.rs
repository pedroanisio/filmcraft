//! Scene documents through import, preferences, the media pool and export.

use std::{path::PathBuf, sync::Arc};

use filmcraft_media::{FrameRequest, SharedSource};
use filmcraft_project::ItemId;
use filmcraft_time::{FrameRate, Tick};
use serde_json::{Value, json};

use crate::{Services, Session, autosave::Preferences};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        Self(crate::media_test_util::tmp_dir("scene-engine"))
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
    fn scene(&self) -> String {
        let path = self.path("clip.SCENE");
        std::fs::write(&path, br#"<scene version="1.7"><project width="32" height="24" fps="24" duration="0.125"/></scene>"#).unwrap();
        path
    }
    fn unavailable(&self, s: &mut Session) {
        // locate falls through a missing preference to PATH. An existing non-executable file
        // wins discovery and fails its probe deterministically, regardless of the VM's PATH.
        let program = self.path("not-a-renderer");
        std::fs::write(&program, "not executable").unwrap();
        let mut prefs = serde_json::to_value(&s.prefs).unwrap();
        prefs["plugins"]["sceneRenderPath"] = json!(program);
        s.set_prefs(serde_json::from_value(prefs).unwrap()).unwrap();
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct BytesOnly;
impl Services for BytesOnly {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }
    fn write_file(&self, path: &str, data: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, data)
    }
    fn file_size(&self, path: &str) -> std::io::Result<u64> {
        Ok(std::fs::metadata(path)?.len())
    }
}

fn import(s: &mut Session, path: &str) -> (ItemId, Value) {
    let result = s.execute("file.import", json!({"paths": [path]})).unwrap();
    assert_eq!(result["items"].as_array().unwrap().len(), 1, "{result}");
    (ItemId(result["items"][0].as_u64().unwrap()), result)
}
fn frame(source: &SharedSource) -> Arc<filmcraft_frame::VideoFrame> {
    source.video_frame(FrameRequest { time: Tick::ZERO, scale: 1.0 }).unwrap()
}

#[test]
fn scene_extension_is_importable_and_in_dialog_filters() {
    assert!(filmcraft_media::is_importable(std::path::Path::new("clip.SCENE")));
    assert!(filmcraft_media::VIDEO_EXTENSIONS.contains(&"scene"));
}

#[test]
fn scene_old_preferences_load_defaults_and_rows_validate() {
    let prefs: Preferences = serde_json::from_value(json!({"plugins":{"developerMode":true}})).unwrap();
    let value = serde_json::to_value(prefs).unwrap();
    assert_eq!(value["plugins"]["sceneRenderPath"], "");
    assert_eq!(value["plugins"]["sceneRenderPreviewQuality"], "preview");
    let mut s = Session::default();
    for quality in ["draft", "preview"] {
        s.execute("prefs.set", json!({"key":"plugins.sceneRenderPreviewQuality", "value":quality})).unwrap();
    }
    for value in [json!("final"), json!(null), json!(-1), json!({})] {
        assert!(s.execute("prefs.set", json!({"key":"plugins.sceneRenderPreviewQuality", "value":value})).is_err());
    }
    s.execute("prefs.set", json!({"key":"plugins.sceneRenderPath", "value":""})).unwrap();
}

fn placeholder_import(reader: bool) {
    let dir = Scratch::new();
    let mut s = Session::default();
    if !reader {
        s.services = Arc::new(BytesOnly);
    }
    dir.unavailable(&mut s);
    let path = dir.scene();
    let (id, result) = import(&mut s, &path);
    assert!(!result["warnings"].as_array().unwrap().is_empty(), "{result}");
    let media = s.project.item(id).unwrap().as_media().unwrap();
    assert_eq!(media.media, filmcraft_project::MediaRef::File { path });
    assert_eq!(media.info.duration, FrameRate::FPS_24.tick_of(3));
    let video = media.info.video.as_ref().unwrap();
    assert_eq!((video.width, video.height, video.frame_rate), (32, 24, FrameRate::FPS_24));
    let src = s.media.source_for(&s.project, id, &*s.services).unwrap();
    let rendered = frame(&src);
    let expected = filmcraft_scenerender::placeholder::slate(32, 24).unwrap();
    assert_eq!(rendered.to_rgba8(), expected.to_rgba8());
    s.media.clear();
    let reopened = s.media.full_res_source(&s.project, id, &*s.services).unwrap();
    assert_eq!(frame(&reopened).to_rgba8(), expected.to_rgba8());
    let bad = dir.path("bad.scene");
    std::fs::write(&bad, "<project/>").unwrap();
    let mixed = s.execute("file.import", json!({"paths":[dir.scene(), bad]})).unwrap();
    assert_eq!(mixed["items"].as_array().unwrap().len(), 1);
    assert_eq!(mixed["errors"].as_array().unwrap().len(), 1);
}
#[test]
fn scene_import_placeholder_with_reader() {
    placeholder_import(true);
}
#[test]
fn scene_import_placeholder_without_reader() {
    placeholder_import(false);
}

#[test]
fn scene_renderer_commands_and_hostile_parameters() {
    let dir = Scratch::new();
    let mut s = Session::default();
    dir.unavailable(&mut s);
    let status = s.execute("scene.renderer.status", json!({})).unwrap();
    assert_eq!(status["found"], false);
    assert!(status["reason"].as_str().is_some());
    for field in ["path", "version", "schema"] {
        assert!(status.get(field).is_some(), "{status}");
    }
    let err = s.execute("scene.renderer.clearCache", json!({})).unwrap_err().to_string();
    assert!(err.contains("cache folder"), "{err}");
    s.execute("prefs.set", json!({"key":"mediaCache.location", "value":dir.path("cache")})).unwrap();
    let cache = dir.0.join("cache/Scene Renders");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("test.bin"), b"12345").unwrap();
    assert_eq!(s.execute("scene.renderer.clearCache", json!({})).unwrap()["bytes"], 5);
    for command in ["scene.renderer.status", "scene.renderer.clearCache"] {
        for params in [json!(null), json!([]), json!("garbage"), json!(i64::MIN), json!({"extra":{"path":"/", "bytes":u64::MAX}})] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(command, params)));
            assert!(result.is_ok(), "{command} panicked");
            assert!(result.unwrap().is_err(), "{command} must reject non-empty/non-object params");
        }
    }
}

#[cfg(unix)]
fn fake_renderer(dir: &Scratch, s: &mut Session) {
    use std::os::unix::fs::PermissionsExt;
    image::RgbaImage::from_pixel(32, 24, image::Rgba([31, 91, 151, 255])).save(dir.0.join("template.png")).unwrap();
    let program = dir.path("scene-render");
    std::fs::write(
        &program,
        r#"#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$root/argv.log"
case "$1" in
--version) echo 'scene-render 0.5.0'; exit 0;;
capabilities) echo '{"format":"scene-render-capabilities/1","engine":{"version":"0.5.0"},"schema":"1.1"}'; exit 0;;
render) shift; shift;;
*) exit 2;;
esac
while [ "$#" -gt 0 ]; do
case "$1" in
--frames) frames=$2; shift 2;;
-o) pattern=$2; shift 2;;
*) shift;;
esac
done
k=${frames%%..*}
end=${frames##*..}
while [ "$k" -lt "$end" ]; do
out=$(printf "$pattern" "$k")
cp "$root/template.png" "$out"
k=$((k + 1))
done
"#,
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    s.execute("prefs.set", json!({"key":"plugins.sceneRenderPath", "value":program})).unwrap();
}

#[cfg(unix)]
fn render_log(dir: &Scratch) -> Vec<String> {
    std::fs::read_to_string(dir.0.join("argv.log")).unwrap().lines().filter(|line| line.starts_with("render ")).map(str::to_owned).collect()
}

#[cfg(unix)]
#[test]
fn scene_stage2_pool_pref_change_invalidates_only_scenes_and_proxies() {
    let dir = Scratch::new();
    let mut s = Session::default();
    fake_renderer(&dir, &mut s);
    let (id, _) = import(&mut s, &dir.scene());
    let src = s.media.source_for(&s.project, id, &*s.services).unwrap();
    let picture = frame(&src);
    assert_eq!((picture.width, picture.height), (32, 24));
    assert_eq!(&picture.to_rgba8()[..4], &[31, 91, 151, 255]);
    let ordinary = import(&mut s, &dir.path("template.png")).0;
    let ordinary_src = s.media.source_for(&s.project, ordinary, &*s.services).unwrap();
    Arc::make_mut(&mut s.project).item_mut(id).unwrap().as_media_mut().unwrap().proxy =
        Some(filmcraft_project::MediaRef::File { path: dir.path("template.png") });
    s.media.set_use_proxies(true);
    let proxy = s.media.source_for(&s.project, id, &*s.services).unwrap();
    let before = s.revision;
    s.execute("prefs.set", json!({"key":"plugins.sceneRenderPreviewQuality", "value":"draft"})).unwrap();
    assert!(s.revision > before);
    let next = s.media.full_res_source(&s.project, id, &*s.services).unwrap();
    assert!(!Arc::ptr_eq(&src, &next), "scene source must reopen for new quality");
    frame(&next);
    assert!(render_log(&dir).iter().any(|line| line.contains("--quality draft")));
    s.media.set_use_proxies(true);
    let next_proxy = s.media.source_for(&s.project, id, &*s.services).unwrap();
    assert!(!Arc::ptr_eq(&proxy, &next_proxy), "scene proxy cache must be dropped");
    assert!(Arc::ptr_eq(&ordinary_src, &s.media.source_for(&s.project, ordinary, &*s.services).unwrap()));
    s.set_prefs(s.prefs.clone()).unwrap();
    assert!(Arc::ptr_eq(&next, &s.media.full_res_source(&s.project, id, &*s.services).unwrap()));
    dir.unavailable(&mut s);
    let changed_program = s.media.full_res_source(&s.project, id, &*s.services).unwrap();
    assert!(!Arc::ptr_eq(&next, &changed_program));
    assert_eq!(frame(&changed_program).to_rgba8(), filmcraft_scenerender::placeholder::slate(32, 24).unwrap().to_rgba8());
}

#[cfg(unix)]
#[test]
fn scene_stage2_cache_clear_and_cache_location_reopen_sources() {
    let dir = Scratch::new();
    let mut s = Session::default();
    fake_renderer(&dir, &mut s);
    s.execute("prefs.set", json!({"key":"mediaCache.location", "value":dir.path("cache")})).unwrap();
    let (id, _) = import(&mut s, &dir.scene());
    let src = s.media.source_for(&s.project, id, &*s.services).unwrap();
    frame(&src);
    let before = s.revision;
    assert!(s.execute("scene.renderer.clearCache", json!({})).unwrap()["bytes"].as_u64().unwrap() > 0);
    let cleared = s.media.source_for(&s.project, id, &*s.services).unwrap();
    assert!(!Arc::ptr_eq(&src, &cleared), "clearCache must drop in-memory frames");
    assert!(s.revision > before);
    frame(&cleared);
    assert_eq!(render_log(&dir).len(), 2);
    s.execute("prefs.set", json!({"key":"mediaCache.location", "value":dir.path("other-cache")})).unwrap();
    assert!(!Arc::ptr_eq(&cleared, &s.media.source_for(&s.project, id, &*s.services).unwrap()));
}

#[cfg(unix)]
fn export_session(dir: &Scratch) -> (Session, ItemId) {
    let mut s = Session::default();
    fake_renderer(dir, &mut s);
    let (id, _) = import(&mut s, &dir.scene());
    s.execute("file.newSequence", json!({"fromItem":id.0})).unwrap();
    (s, id)
}

#[cfg(unix)]
#[test]
fn scene_stage2_export_media_uses_final_while_playback_and_previews_use_preview() {
    use filmcraft_render::SourceProvider;
    let dir = Scratch::new();
    let (mut s, id) = export_session(&dir);
    frame(&s.media.source_for(&s.project, id, &*s.services).unwrap());
    let preview = s.media.full_res_provider(s.project.clone(), s.services.clone());
    frame(&preview.source(id).unwrap());
    assert_eq!(render_log(&dir).len(), 1);
    assert!(render_log(&dir)[0].contains("--quality preview"));
    let result = s.execute("file.exportMedia", json!({"path":dir.path("export.png"), "format":"png", "wait":true})).unwrap();
    assert!(result["offlineMedia"].as_array().unwrap().is_empty(), "{result}");
    assert!(render_log(&dir).iter().any(|line| line.contains("--quality final")), "export must render final frames: {:?}", render_log(&dir));
    let count = render_log(&dir).len();
    frame(&preview.source(id).unwrap());
    assert_eq!(render_log(&dir).len(), count, "preview retains its own source");
}

#[cfg(unix)]
#[test]
fn scene_stage2_export_queue_uses_final() {
    let dir = Scratch::new();
    let (mut s, _) = export_session(&dir);
    s.execute("export.queue.add", json!({"path":dir.path("queue.png"), "format":"png"})).unwrap();
    s.execute("export.queue.start", json!({"wait":true})).unwrap();
    assert!(render_log(&dir).iter().any(|line| line.contains("--quality final")), "queue must render final frames");
}

#[cfg(unix)]
#[test]
fn scene_stage2_export_frame_uses_final() {
    let dir = Scratch::new();
    let (mut s, _) = export_session(&dir);
    s.execute("file.exportFrame", json!({"path":dir.path("frame.png"), "format":"png", "target":"program"})).unwrap();
    assert!(render_log(&dir).iter().any(|line| line.contains("--quality final")), "still export must render final frames");
}

#[test]
fn scene_stage2_export_placeholder_reports_after_pool_reopen() {
    let dir = Scratch::new();
    let mut s = Session::default();
    dir.unavailable(&mut s);
    let (id, _) = import(&mut s, &dir.scene());
    s.execute("file.newSequence", json!({"fromItem":id.0})).unwrap();
    s.media.clear();
    let result = s.execute("file.exportMedia", json!({"path":dir.path("placeholder.png"), "format":"png", "wait":true})).unwrap();
    assert_eq!(result["offlineMedia"][0]["item"], id.0, "export must report placeholder: {result}");
    assert!(result["offlineMedia"][0]["error"].as_str().unwrap().contains("scene-render"));
}

#[cfg(unix)]
#[test]
fn scene_stage3_aaf_mixdown_uses_final_and_reports_placeholders() {
    let dir = Scratch::new();
    let (mut s, id) = export_session(&dir);
    s.execute("file.exportAaf", json!({"path":dir.path("final.aaf"), "mixdownVideo":true})).unwrap();
    assert!(render_log(&dir).iter().any(|line| line.contains("--quality final")), "AAF mixdown must render final frames");
    dir.unavailable(&mut s);
    let result = s.execute("file.exportAaf", json!({"path":dir.path("placeholder.aaf"), "mixdownVideo":true})).unwrap();
    assert_eq!(result["offlineMedia"][0]["item"], id.0, "{result}");
}

#[cfg(unix)]
#[test]
fn scene_stage3_consolidation_uses_final() {
    let dir = Scratch::new();
    let (mut s, _) = export_session(&dir);
    s.execute("file.projectManager", json!({"destination":dir.path("consolidated"),"mode":"consolidate","handles":0,"preset":"prores_lt","wait":true}))
        .unwrap();
    assert!(render_log(&dir).iter().any(|line| line.contains("--quality final")), "consolidated media must render final frames");
}

#[cfg(unix)]
#[test]
fn scene_stage3_transcode_uses_final() {
    let dir = Scratch::new();
    let (mut s, id) = export_session(&dir);
    crate::proxies::start_job(
        &mut s,
        "Scene transcode".into(),
        vec![(id, dir.path("transcode.mov"), crate::proxies::preset("prores_lt").unwrap())],
        crate::proxies::OnDone::ReplaceMedia,
        true,
    )
    .unwrap();
    assert!(render_log(&dir).iter().any(|line| line.contains("--quality final")), "transcoded media must render final frames");
}

#[cfg(unix)]
#[test]
fn scene_startup_applies_saved_renderer_quality_and_cache() {
    let dir = Scratch::new();
    let mut configured = Session::default();
    fake_renderer(&dir, &mut configured);
    configured.execute("prefs.set", json!({"key":"plugins.sceneRenderPreviewQuality", "value":"draft"})).unwrap();
    configured.prefs.save(&dir.0.join("preferences.json")).unwrap();
    let mut s = Session::default();
    s.start_autosave(crate::autosave::AutosaveConfig::new(&dir.0)).unwrap();
    let (id, _) = import(&mut s, &dir.scene());
    frame(&s.media.source_for(&s.project, id, &*s.services).unwrap());
    assert!(render_log(&dir)[0].contains("--quality draft"));
    assert!(dir.0.join("Media Cache/Scene Renders").is_dir());
    s.shutdown();
}

#[test]
fn scene_hostile_reader_length_is_rejected_before_reading() {
    struct Huge;
    impl filmcraft_media::ByteReader for Huge {
        fn len(&self) -> u64 {
            u64::MAX
        }
        fn read_at(&self, _: u64, _: &mut [u8]) -> std::io::Result<()> {
            panic!("oversized scene must never be read")
        }
    }
    impl Services for Huge {
        fn read_file(&self, _: &str) -> std::io::Result<Vec<u8>> {
            panic!("must use reader")
        }
        fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
            panic!("must not write")
        }
        fn reader(&self, _: &str) -> Option<std::io::Result<filmcraft_media::SharedReader>> {
            Some(Ok(Arc::new(Huge)))
        }
    }
    let mut s = Session { services: Arc::new(Huge), ..Session::default() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute("file.import", json!({"path":"/huge.scene"}))));
    assert!(result.is_ok());
    assert!(result.unwrap().unwrap_err().to_string().contains("exceeds"));
}
