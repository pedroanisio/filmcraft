//! Native/hybrid scene command contracts through an in-memory host.

use crate::{Services, Session};
use filmcraft_project::{ItemId, MediaRef};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Files(Mutex<HashMap<String, Vec<u8>>>);
impl Services for Files {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.0.lock().unwrap().get(path).cloned().ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }
    fn write_file(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
        self.0.lock().unwrap().insert(path.into(), bytes.into());
        Ok(())
    }
    fn export_in_memory(&self) -> bool {
        true
    }
}
fn xml(overlay: bool) -> String {
    let nodes = if overlay { r#"<particleEmitter id="particles" start="1" end="3"/>"# } else { "" };
    format!(r#"<scene version="1.7"><project width="32" height="24" fps="24" duration="4"/><composition>{nodes}</composition></scene>"#)
}
fn setup(overlay: bool) -> (Session, Arc<Files>) {
    let files = Arc::new(Files::default());
    files.write_file("/virtual/demo.scene", xml(overlay).as_bytes()).unwrap();
    let mut s = Session { services: files.clone(), ..Default::default() };
    // An existing non-program wins discovery even if scene-render is in PATH.
    let mut prefs = serde_json::to_value(&s.prefs).unwrap();
    prefs["plugins"]["sceneRenderPath"] = json!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    s.set_prefs(serde_json::from_value(prefs).unwrap()).unwrap();
    (s, files)
}
fn sequence(r: &Value) -> ItemId {
    ItemId(r["sequences"][0].as_u64().unwrap())
}

#[test]
fn scene_interchange_detection() {
    for path in ["demo.scene", "demo.SCENE", "demo.xml"] {
        assert_eq!(super::detect(path, xml(false).as_bytes()), Some(filmcraft_interchange::Format::Scene));
    }
}
#[test]
fn scene_interchange_native_simple_and_one_undo() {
    let (mut s, _) = setup(false);
    let before = s.project.items.len();
    let undo = s.history.undo.len();
    let r = s.execute("file.importScene", json!({"path":"/virtual/demo.scene", "mode":"native"})).unwrap();
    let q = s.project.sequence(sequence(&r)).unwrap();
    assert_eq!(q.video_tracks.len(), 1);
    assert_eq!(q.video_tracks[0].items.len(), 1);
    assert_eq!(q.duration(), Tick(4 * TICKS_PER_SECOND));
    assert_eq!(r["overlays"], json!([]));
    assert_eq!(s.history.undo.len(), undo + 1);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.items.len(), before);
}
#[test]
fn scene_interchange_native_refuses_or_reports_dropped_overlay() {
    let (mut s, files) = setup(true);
    let before = s.project.items.len();
    let error = s.execute("file.importScene", json!({"path":"/virtual/demo.scene", "mode":"native"})).unwrap_err();
    assert!(error.to_string().contains("1 scene overlay"), "{error}");
    assert_eq!(s.project.items.len(), before);
    let r = s.execute("file.importScene", json!({"path":"/virtual/demo.scene", "mode":"native", "lenient":true})).unwrap();
    assert!(r["report"].as_array().unwrap().iter().any(|v| v.as_str().unwrap().contains("dropped")));
    assert_eq!(r["overlays"], json!([]));
    assert_eq!(files.0.lock().unwrap().len(), 1);
    assert!(s.project.sequence(sequence(&r)).unwrap().video_tracks.iter().flat_map(|t| &t.items).all(|c| c.name != "particles"));
}
#[test]
fn scene_interchange_hybrid_overlay_links_and_renders_placeholder() {
    for mode in [None, Some("hybrid")] {
        let (mut s, files) = setup(true);
        let before = s.project.items.len();
        let undo = s.history.undo.len();
        let mut params = json!({"path":"/virtual/demo.scene"});
        if let Some(mode) = mode {
            params["mode"] = json!(mode);
        }
        let r = s.execute("file.importScene", params).unwrap();
        assert_eq!(r["overlays"].as_array().unwrap().len(), 1);
        let item = ItemId(r["overlays"][0]["item"].as_u64().unwrap());
        let path = r["overlays"][0]["path"].as_str().unwrap();
        assert!(path.ends_with(".scene"));
        let bytes = files.read_file(path).unwrap();
        let doc = filmcraft_interchange::scene::read::read(std::str::from_utf8(&bytes).unwrap(), &Default::default()).unwrap();
        assert!(filmcraft_interchange::scene::validate::validate(&doc.doc).iter().all(|d| !d.is_error()));
        assert_eq!(s.project.item(item).unwrap().as_media().unwrap().media, MediaRef::File { path: path.into() });
        let q = s.project.sequence(sequence(&r)).unwrap();
        let c = &q.video_tracks[1].items[0];
        assert_eq!(c.item, item);
        assert_eq!((c.start, c.duration, c.source_in), (Tick(TICKS_PER_SECOND), Tick(2 * TICKS_PER_SECOND), Tick(TICKS_PER_SECOND)));
        assert_eq!(r["linkedMedia"], 1);
        let actual = s.source(item).unwrap().video_frame(filmcraft_media::FrameRequest { time: c.source_in, scale: 1.0 }).unwrap();
        let expected = filmcraft_scenerender::placeholder::slate(32, 24).unwrap();
        assert_eq!(actual.to_rgba8(), expected.to_rgba8());
        let program = s.render_program_at(1.0, Tick(2 * TICKS_PER_SECOND)).unwrap();
        assert_eq!(program.to_rgba8(), expected.to_rgba8());
        assert!(s.media.offline_status(item).is_some());
        assert_eq!(s.history.undo.len(), undo + 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.project.items.len(), before);
        let again = s.execute("file.importScene", json!({"path":"/virtual/demo.scene"})).unwrap();
        assert_eq!(again["overlays"].as_array().unwrap().len(), 1);
        assert_eq!(files.0.lock().unwrap().len(), 2);
    }
}
#[test]
fn scene_interchange_rendered_preserves_file_import() {
    for command in ["file.import", "file.importScene"] {
        let (mut s, _) = setup(true);
        let before = s.project.items.len();
        let p = if command == "file.import" { json!({"paths":["/virtual/demo.scene"]}) } else { json!({"path":"/virtual/demo.scene","mode":"rendered"}) };
        let r = s.execute(command, p).unwrap();
        assert_eq!(r["items"].as_array().unwrap().len(), 1);
        assert_eq!(s.project.items.len(), before + 1);
        let item = ItemId(r["items"][0].as_u64().unwrap());
        assert!(matches!(s.project.item(item).unwrap().as_media().unwrap().media, MediaRef::File { .. }));
    }
}
#[test]
fn scene_interchange_export_valid_scene_and_registry() {
    let (mut s, files) = setup(false);
    s.execute("file.importScene", json!({"path":"/virtual/demo.scene","mode":"native"})).unwrap();
    for cmd in ["file.exportScene", "file.exportInterchange"] {
        s.execute(cmd, json!({"path":"/virtual/export.scene","format":"scene"})).unwrap();
        let bytes = files.read_file("/virtual/export.scene").unwrap();
        let doc = filmcraft_interchange::scene::read::read(std::str::from_utf8(&bytes).unwrap(), &Default::default()).unwrap();
        assert!(filmcraft_interchange::scene::validate::validate(&doc.doc).iter().all(|d| !d.is_error()));
    }
    let spec = crate::commands::find("file.importScene").unwrap();
    assert_eq!(spec.menu, &["File", "Import"]);
    assert!(spec.params.contains("lenient"));
    assert!((spec.enabled)(&s).is_ok());
    assert_eq!(crate::commands::find("file.exportScene").unwrap().label, "sr-core scene (.scene)");
}
#[test]
fn scene_interchange_hostile_params() {
    let (mut s, files) = setup(false);
    let before = s.project.clone();
    for p in [
        json!(null),
        json!([]),
        json!({}),
        json!({"path":4}),
        json!({"path":""}),
        json!({"path":"../demo.scene"}),
        json!({"path":"/virtual/../demo.scene"}),
        json!({"path":"bad\u{0000}.scene"}),
        json!({"path":"x".repeat(17000)}),
        json!({"path":"/virtual/demo.scene","unknown":true}),
    ] {
        assert!(s.execute("file.importScene", p.clone()).is_err(), "accepted {p}");
    }
    for key in ["mode", "lenient"] {
        for v in [json!(null), json!(1), json!([]), json!({}), json!("x".repeat(17000)), json!("../hybrid"), json!("hybrid\u{0000}")] {
            let mut p = json!({"path":"/virtual/demo.scene"});
            p[key] = v;
            assert!(s.execute("file.importScene", p).is_err());
        }
    }
    for v in [json!(false), json!(true), json!("NATIVE")] {
        assert!(s.execute("file.importScene", json!({"path":"/virtual/demo.scene","mode":v})).is_err());
    }
    assert!(Arc::ptr_eq(&s.project, &before));
    assert_eq!(files.0.lock().unwrap().len(), 1);
    for v in [json!(null), json!("true"), json!(1), json!([]), json!({}), json!("x".repeat(17000))] {
        assert!(s.execute("file.import", json!({"paths":["/virtual/demo.scene"],"lenient":v})).is_err());
    }
}

#[test]
fn scene_interchange_xml_lenient_reaches_import_options() {
    let (mut s, files) = setup(true);
    files.write_file("/virtual/demo.xml", xml(true).as_bytes()).unwrap();
    let error = s.execute("file.import", json!({"paths":["/virtual/demo.xml"]})).unwrap_err();
    assert!(error.to_string().contains("1 scene overlay"), "{error}");
    let r = s.execute("file.import", json!({"paths":["/virtual/demo.xml"],"lenient":true})).unwrap();
    assert!(r["documents"][0]["report"].as_array().unwrap().iter().any(|v| v.as_str().unwrap().contains("dropped")));
}

#[test]
fn scene_interchange_write_failure_preserves_project() {
    struct ReadOnly;
    impl Services for ReadOnly {
        fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
            if path == "/virtual/demo.scene" { Ok(xml(true).into_bytes()) } else { Err(std::io::ErrorKind::NotFound.into()) }
        }
        fn write_file(&self, _: &str, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::other("disk full"))
        }
        fn export_in_memory(&self) -> bool {
            true
        }
    }
    let mut s = Session { services: Arc::new(ReadOnly), ..Default::default() };
    let before = s.project.clone();
    let undo = s.history.undo.len();
    let error = s.execute("file.importScene", json!({"path":"/virtual/demo.scene"})).unwrap_err();
    assert!(error.to_string().contains("disk full"), "{error}");
    assert!(Arc::ptr_eq(&s.project, &before));
    assert_eq!(s.history.undo.len(), undo);
}

#[test]
fn scene_interchange_export_hostile_params_do_not_write() {
    let (mut s, files) = setup(false);
    s.execute("file.importScene", json!({"path":"/virtual/demo.scene", "mode":"native"})).unwrap();
    for command in ["file.exportScene", "file.exportInterchange"] {
        for path in [
            json!(null),
            json!(2),
            json!([]),
            json!({}),
            json!(""),
            json!("../out.scene"),
            json!("/virtual/../out.scene"),
            json!("bad\u{0000}.scene"),
            json!("x".repeat(17000)),
        ] {
            assert!(s.execute(command, json!({"path":path,"format":"scene"})).is_err());
        }
        for sequence in [json!(null), json!(-1), json!(1.5), json!("0"), json!("x".repeat(17000)), json!([]), json!({})] {
            assert!(s.execute(command, json!({"path":"/virtual/out.scene","format":"scene","sequence":sequence})).is_err());
        }
    }
    assert_eq!(files.0.lock().unwrap().len(), 1);
}

#[test]
fn scene_interchange_hybrid_preserves_existing_overlay_file() {
    let (mut s, files) = setup(true);
    let (_, overlays, _) = filmcraft_interchange::scene::import(
        xml(true).as_bytes(),
        &filmcraft_interchange::ImportOptions { base_dir: Some("/virtual".into()), name: Some("demo".into()), ..Default::default() },
    )
    .unwrap();
    let path = &overlays[0].path;
    files.write_file(path, b"existing user work").unwrap();
    let before = s.project.clone();
    let error = s.execute("file.importScene", json!({"path":"/virtual/demo.scene"})).unwrap_err();
    assert!(error.to_string().contains("exists"), "{error}");
    assert_eq!(files.read_file(path).unwrap(), b"existing user work");
    assert!(Arc::ptr_eq(&before, &s.project));
}

#[test]
fn scene_fix_export_registry_has_no_duplicate() {
    assert!(crate::commands::find("file.export").is_none());
}

#[test]
fn scene_fix_versioned_names_do_not_collide() {
    let (mut s, files) = setup(true);
    let mut paths = Vec::new();
    for version in [1, 2] {
        let path = format!("/virtual/shot.v{version}.scene");
        let document = xml(true).replace("end=\"3\"", &format!("end=\"{}\"", version + 1));
        files.write_file(&path, document.as_bytes()).unwrap();
        let r = s.execute("file.importScene", json!({"path":path})).unwrap();
        paths.push(r["overlays"][0]["path"].as_str().unwrap().to_string());
        assert_eq!(s.project.item(sequence(&r)).unwrap().name, format!("shot.v{version}"));
    }
    assert_ne!(paths[0], paths[1]);
    assert!(paths[0].contains("shot_v1 Scene Overlays"));
    assert!(paths[1].contains("shot_v2 Scene Overlays"));
    // The generic XML import must also strip only once.
    files.write_file("/virtual/shot.v3.xml", xml(false).as_bytes()).unwrap();
    let r = s.execute("file.import", json!({"paths":["/virtual/shot.v3.xml"]})).unwrap();
    assert_eq!(s.project.item(sequence(&r)).unwrap().name, "shot.v3");
}

#[test]
fn scene_fix_invalid_diagnostics_survive_engine() {
    let (mut s, files) = setup(false);
    for (command, params, path) in [
        ("file.importScene", json!({"path":"/virtual/bad.scene","mode":"native"}), "/virtual/bad.scene"),
        ("file.importScene", json!({"path":"/virtual/bad.scene"}), "/virtual/bad.scene"),
        ("file.import", json!({"paths":["/virtual/bad.xml"]}), "/virtual/bad.xml"),
    ] {
        files.write_file(path, xml(false).replace("width=\"32\"", "width=\"0\"").as_bytes()).unwrap();
        let error = s.execute(command, params).unwrap_err();
        let v = error.to_json();
        assert_eq!(v["error"], error.to_string());
        let d = v["diagnostics"].as_array().expect("structured diagnostics");
        assert!(!d.is_empty());
        for d in d {
            assert_eq!(d.as_object().unwrap().len(), 5);
            assert!(d["severity"].is_string());
            assert!(!d["rule"].as_str().unwrap().is_empty());
            assert!(d["line"].as_u64().unwrap() > 0);
            assert!(d["path"].as_str().unwrap().starts_with("/scene"));
            assert!(d["message"].is_string());
        }
    }
    assert_eq!(crate::EngineError::Other("unchanged".into()).to_json(), json!({"error":"unchanged"}));
}

#[cfg(unix)]
fn overlay_renderer_session(virtual_files: bool) -> (Session, std::path::PathBuf, String) {
    use std::os::unix::fs::PermissionsExt;
    let dir = crate::media_test_util::tmp_dir("scene-overlay-service");
    let program = dir.join("scene-render");
    // Check the actual document is readable before producing a frame.
    std::fs::write(
        &program,
        r#"#!/bin/sh
set -eu
root=$(dirname "$0")
printf '%s\n' "$*" >> "$root/argv.log"
case "$1" in
--version) echo 'scene-render 0.5.0'; exit 0;;
capabilities) echo '{"format":"scene-render-capabilities/1","engine":{"version":"0.5.0"},"schema":"1.1"}'; exit 0;;
render) test -f "$2"; shift 2;;
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
    image::RgbaImage::from_pixel(32, 24, image::Rgba([31, 91, 151, 255])).save(dir.join("template.png")).unwrap();
    let services: Arc<dyn Services> = if virtual_files { Arc::new(Files::default()) } else { Arc::new(crate::FsServices) };
    let path = dir.join("shot.scene").to_string_lossy().into_owned();
    services.write_file(&path, xml(true).as_bytes()).unwrap();
    let mut s = Session { services, ..Default::default() };
    s.execute("prefs.set", json!({"key":"plugins.sceneRenderPath","value":program})).unwrap();
    (s, dir, path)
}

#[cfg(unix)]
#[test]
fn scene_fix_virtual_overlays_never_spawn_renderer() {
    let (mut s, dir, path) = overlay_renderer_session(true);
    let r = s.execute("file.importScene", json!({"path":path})).unwrap();
    assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("filesystem-backed")), "{r}");
    let id = ItemId(r["overlays"][0]["item"].as_u64().unwrap());
    let frame = s.source(id).unwrap().video_frame(filmcraft_media::FrameRequest::full(Tick(TICKS_PER_SECOND))).unwrap();
    assert_eq!(frame.to_rgba8(), filmcraft_scenerender::placeholder::slate(32, 24).unwrap().to_rgba8());
    assert!(!dir.join("argv.log").exists(), "virtual overlays must not probe or render externally");
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn scene_fix_filesystem_overlays_use_renderer() {
    let (mut s, dir, path) = overlay_renderer_session(false);
    let r = s.execute("file.importScene", json!({"path":path})).unwrap();
    assert_eq!(r["warnings"], json!([]));
    let id = ItemId(r["overlays"][0]["item"].as_u64().unwrap());
    let frame = s.source(id).unwrap().video_frame(filmcraft_media::FrameRequest::full(Tick(TICKS_PER_SECOND))).unwrap();
    assert_eq!(frame.to_rgba8(), [31, 91, 151, 255].repeat(32 * 24));
    assert!(std::fs::read_to_string(dir.join("argv.log")).unwrap().contains("render "));
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn scene_fix_virtual_overlays_stay_placeholder_after_cache_clear() {
    let (mut s, dir, path) = overlay_renderer_session(true);
    let r = s.execute("file.importScene", json!({"path":path})).unwrap();
    let id = ItemId(r["overlays"][0]["item"].as_u64().unwrap());
    s.media.clear_scenes();
    let actual = s.source(id).unwrap().video_frame(filmcraft_media::FrameRequest::full(Tick(TICKS_PER_SECOND))).unwrap();
    assert!(actual.to_rgba8() == filmcraft_scenerender::placeholder::slate(32, 24).unwrap().to_rgba8(), "expected scene placeholder slate");
    assert!(!dir.join("argv.log").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn scene_fix_virtual_overlays_stay_placeholder_for_export() {
    use filmcraft_render::SourceProvider;
    let (mut s, dir, path) = overlay_renderer_session(true);
    let r = s.execute("file.importScene", json!({"path":path})).unwrap();
    let id = ItemId(r["overlays"][0]["item"].as_u64().unwrap());
    let provider = s.media.provider(s.project.clone(), s.services.clone()).final_scenes().recording();
    let actual = provider.source(id).unwrap().video_frame(filmcraft_media::FrameRequest::full(Tick(TICKS_PER_SECOND))).unwrap();
    assert!(actual.to_rgba8() == filmcraft_scenerender::placeholder::slate(32, 24).unwrap().to_rgba8(), "expected scene placeholder slate");
    assert!(provider.offline_used()[0].error.contains("filesystem-backed"));
    assert!(!dir.join("argv.log").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn scene_fix_filesystem_overlays_use_renderer_for_export() {
    use filmcraft_render::SourceProvider;
    struct DiskWithMemoryExports;
    impl Services for DiskWithMemoryExports {
        fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
            crate::FsServices.read_file(path)
        }
        fn write_file(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
            crate::FsServices.write_file(path, bytes)
        }
        fn is_filesystem_backed(&self, path: &str) -> bool {
            crate::FsServices.is_filesystem_backed(path)
        }
        fn export_in_memory(&self) -> bool {
            true
        }
    }
    for encode_in_memory in [false, true] {
        let (mut s, dir, path) = overlay_renderer_session(false);
        if encode_in_memory {
            s.services = Arc::new(DiskWithMemoryExports);
        }
        let r = s.execute("file.importScene", json!({"path":path})).unwrap();
        assert_eq!(r["warnings"], json!([]));
        let id = ItemId(r["overlays"][0]["item"].as_u64().unwrap());
        s.media.clear_scenes();
        let provider = s.media.provider(s.project.clone(), s.services.clone()).final_scenes().recording();
        let actual = provider.source(id).unwrap().video_frame(filmcraft_media::FrameRequest::full(Tick(TICKS_PER_SECOND))).unwrap();
        assert_eq!(actual.to_rgba8(), [31, 91, 151, 255].repeat(32 * 24));
        assert!(provider.offline_used().is_empty());
        let argv = std::fs::read_to_string(dir.join("argv.log")).unwrap();
        assert!(argv.contains("render ") && argv.contains("--quality final"), "{argv}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn scene_fix_virtual_storage_independent_of_export_encoding() {
    struct VirtualWithStreamingExports(Arc<dyn Services>);
    impl Services for VirtualWithStreamingExports {
        fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
            self.0.read_file(path)
        }
        fn write_file(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
            self.0.write_file(path, bytes)
        }
    }
    let (mut s, dir, path) = overlay_renderer_session(true);
    s.services = Arc::new(VirtualWithStreamingExports(s.services.clone()));
    let r = s.execute("file.importScene", json!({"path":path})).unwrap();
    assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("filesystem-backed")), "{r}");
    let overlay_path = std::path::Path::new(r["overlays"][0]["path"].as_str().unwrap());
    assert!(!overlay_path.parent().unwrap().exists(), "virtual overlay must not create a native directory");
    let reopened = s.media.open_file(overlay_path.to_str().unwrap(), &*s.services).unwrap();
    let actual = reopened.video_frame(filmcraft_media::FrameRequest::full(Tick(TICKS_PER_SECOND))).unwrap();
    assert_eq!(actual.to_rgba8(), filmcraft_scenerender::placeholder::slate(32, 24).unwrap().to_rgba8());
    assert!(!dir.join("argv.log").exists());
    std::fs::remove_dir_all(dir).unwrap();
}
