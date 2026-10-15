//! Package command contracts with synthetic scenes and an in-memory host.
use crate::{EngineError, Services, Session};
use filmcraft_project::ItemId;
use filmcraft_vpkg::{FileInput, Limits, PackInput, manifest::Metadata};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Files {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    writes: Mutex<usize>,
    fail_write: Mutex<bool>,
    oversized: Mutex<bool>,
    reads: Mutex<Vec<String>>,
    fail_after: Mutex<Option<usize>>,
}
impl Services for Files {
    fn read_file(&self, path: &str) -> std::io::Result<Vec<u8>> {
        self.reads.lock().unwrap().push(path.into());
        assert!(!*self.oversized.lock().unwrap(), "oversized file must not be read");
        self.files.lock().unwrap().get(path).cloned().ok_or(std::io::ErrorKind::NotFound.into())
    }
    fn file_size(&self, path: &str) -> std::io::Result<u64> {
        if *self.oversized.lock().unwrap() {
            return Ok(u64::MAX);
        }
        self.files.lock().unwrap().get(path).map(|v| v.len() as u64).ok_or(std::io::ErrorKind::NotFound.into())
    }
    fn write_file(&self, path: &str, bytes: &[u8]) -> std::io::Result<()> {
        let mut remaining = self.fail_after.lock().unwrap();
        if let Some(count) = remaining.as_mut() {
            if *count == 0 {
                return Err(std::io::Error::other("disk full"));
            }
            *count -= 1;
        }
        if *self.fail_write.lock().unwrap() {
            return Err(std::io::Error::other("disk full"));
        }
        *self.writes.lock().unwrap() += 1;
        self.files.lock().unwrap().insert(path.into(), bytes.into());
        Ok(())
    }
    fn list_dir(&self, dir: &str) -> Option<std::io::Result<Vec<String>>> {
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        Some(Ok(self.files.lock().unwrap().keys().filter_map(|p| p.strip_prefix(&prefix).map(str::to_string)).collect()))
    }
    fn export_in_memory(&self) -> bool {
        true
    }
}
fn setup() -> (Session, Arc<Files>) {
    let files = Arc::new(Files::default());
    let s = Session { services: files.clone(), ..Default::default() };
    (s, files)
}
fn scene() -> &'static str {
    // 24 fps × 1 second: one full-length image and the default background matte.
    r#"<scene version="1.7"><project width="32" height="24" fps="24" duration="1"/><assets><image id="img" src="assets/pixel.png" width="1" height="1"/></assets><composition><layer id="picture" asset="img" start="0" end="1"/></composition></scene>"#
}
fn png() -> Vec<u8> {
    let mut data = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(1, 1, image::Rgba([31, 91, 151, 255])).write_to(&mut data, image::ImageFormat::Png).unwrap();
    data.into_inner()
}
fn package() -> Vec<u8> {
    filmcraft_vpkg::pack(&PackInput {
        primary: FileInput { path: "main.scene".into(), bytes: scene().as_bytes().into(), role: "scene".into() },
        files: vec![FileInput { path: "assets/pixel.png".into(), bytes: png(), role: "asset".into() }],
        metadata: Metadata { id: "demo".into(), version: "1.0.0".into(), title: "Demo".into(), created: None },
        limits: Limits::default(),
        source_date_epoch: None,
    })
    .unwrap()
}
fn load(s: &mut Session, files: &Files) -> Value {
    files.write_file("/virtual/demo.vpkg.zip", &package()).unwrap();
    s.execute("file.importVpkg", json!({"path":"/virtual/demo.vpkg.zip","mode":"native"})).unwrap()
}
fn count(s: &Session, r: &Value) -> usize {
    let seq = s.project.sequence(ItemId(r["sequences"][0].as_u64().unwrap())).unwrap();
    seq.video_tracks.iter().chain(&seq.audio_tracks).map(|t| t.items.len()).sum()
}
#[test]
fn vpkg_import_fixtures_and_one_undo() {
    for bytes in [
        include_bytes!("../../vpkg/tests/fixtures/minimal.vpkg.zip").as_slice(),
        include_bytes!("../../vpkg/tests/fixtures/includes.vpkg.zip").as_slice(),
        include_bytes!("../../vpkg/tests/fixtures/fetch.vpkg.zip").as_slice(),
    ] {
        let (mut s, files) = setup();
        files.write_file("/virtual/demo.vpkg.zip", bytes).unwrap();
        let before = s.project.clone();
        let undo = s.history.undo.len();
        let r = s.execute("file.importVpkg", json!({"path":"/virtual/demo.vpkg.zip","lenient":true})).unwrap();
        assert!(r["package"]["id"].is_string(), "{r}");
        assert_eq!(r["package"]["extractedTo"], "/virtual/demo");
        let primary = r["package"]["primaryScene"].as_str().unwrap();
        assert!(files.read_file(&format!("/virtual/demo/project/{primary}")).is_ok());
        assert_eq!(s.history.undo.len(), undo + 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert!(Arc::ptr_eq(&before, &s.project));
    }
}
#[test]
fn vpkg_tampered_hash_writes_nothing() {
    let (mut s, files) = setup();
    let mut entries = filmcraft_vpkg::zip::read(&package(), &Limits::default()).unwrap();
    entries.iter_mut().find(|e| e.path.ends_with("pixel.png")).unwrap().bytes[0] ^= 1;
    let inputs: Vec<_> = entries.iter().map(|e| filmcraft_vpkg::zip::Input { path: &e.path, bytes: &e.bytes, executable: false }).collect();
    let bytes = filmcraft_vpkg::zip::write(&inputs, &Limits::default(), None).unwrap();
    files.write_file("/virtual/bad.zip", &bytes).unwrap();
    let before = s.project.clone();
    let writes = *files.writes.lock().unwrap();
    let e = s.execute("file.importVpkg", json!({"path":"/virtual/bad.zip"})).unwrap_err();
    assert!(e.to_json()["diagnostics"].as_array().unwrap().iter().any(|d| d["rule"] == "HASH"));
    assert_eq!(*files.writes.lock().unwrap(), writes);
    assert!(Arc::ptr_eq(&before, &s.project));
}
#[test]
fn vpkg_collision_and_write_failure_preserve_project() {
    for default_dir in [false, true] {
        let (mut s, files) = setup();
        files.write_file("/virtual/demo.vpkg.zip", &package()).unwrap();
        let collision = if default_dir { "/virtual/demo/unrelated.txt" } else { "/virtual/out/project/assets/pixel.png" };
        files.write_file(collision, b"user work").unwrap();
        let before = s.project.clone();
        let writes = *files.writes.lock().unwrap();
        let p = if default_dir { json!({"path":"/virtual/demo.vpkg.zip"}) } else { json!({"path":"/virtual/demo.vpkg.zip","dir":"/virtual/out"}) };
        assert!(s.execute("file.importVpkg", p).is_err());
        assert!(Arc::ptr_eq(&before, &s.project));
        assert_eq!(*files.writes.lock().unwrap(), writes);
        assert_eq!(files.read_file(collision).unwrap(), b"user work");
    }
    let (mut s, files) = setup();
    files.write_file("/virtual/demo.zip", &package()).unwrap();
    *files.fail_write.lock().unwrap() = true;
    let before = s.project.clone();
    let e = s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip"})).unwrap_err();
    assert!(e.to_string().contains("disk full"));
    assert!(Arc::ptr_eq(&before, &s.project));
}
#[test]
fn vpkg_round_trip_media_and_clip_count() {
    let (mut s, files) = setup();
    let r = load(&mut s, &files);
    assert_eq!(count(&s, &r), 2);
    let out = s.execute("file.exportVpkg", json!({"path":"/virtual/out.vpkg.zip","id":"round-trip","title":"Round Trip"})).unwrap();
    assert!(out["bytes"].as_u64().unwrap() > 0);
    let packed = files.read_file("/virtual/out.vpkg.zip").unwrap();
    let p = filmcraft_vpkg::read(&packed, &Limits::default()).unwrap();
    assert_eq!(p.manifest.data.package.id, "round-trip");
    assert_eq!(p.manifest.data.package.version, "1.0.0");
    assert_eq!(p.manifest.data.package.title, "Round Trip");
    assert!(p.entries.iter().any(|e| e.path.starts_with("project/assets/") && e.bytes == png()));
    let r2 = s.execute("file.importVpkg", json!({"path":"/virtual/out.vpkg.zip","mode":"native"})).unwrap();
    assert_eq!(count(&s, &r2), 2);
    s.execute("file.exportVpkg", json!({"path":"/virtual/again.zip","sequence":r["sequences"][0],"id":"round-trip","title":"Round Trip"})).unwrap();
    assert_eq!(files.read_file("/virtual/again.zip").unwrap(), packed);
}
#[test]
fn vpkg_missing_media_is_error() {
    let (mut s, files) = setup();
    load(&mut s, &files);
    files.files.lock().unwrap().remove("/virtual/demo/project/assets/pixel.png");
    let e = s.execute("file.exportVpkg", json!({"path":"/virtual/out.zip"})).unwrap_err();
    assert!(e.to_string().contains("pixel.png"), "{e}");
    assert!(files.read_file("/virtual/out.zip").is_err());
}
#[test]
fn vpkg_hostile_params_and_size() {
    let (mut s, files) = setup();
    // Make export enabled independently of the package import implementation.
    files.write_file("/virtual/main.scene", scene().as_bytes()).unwrap();
    s.execute("file.importScene", json!({"path":"/virtual/main.scene","mode":"native"})).unwrap();
    for cmd in ["file.importVpkg", "file.exportVpkg"] {
        for p in [
            json!(null),
            json!([]),
            json!({}),
            json!({"path":2}),
            json!({"path":""}),
            json!({"path":"../bad"}),
            json!({"path":"a\u{0000}"}),
            json!({"path":"x".repeat(17000)}),
            json!({"path":"/virtual/a.zip","unknown":true}),
        ] {
            assert!(matches!(s.execute(cmd, p.clone()), Err(EngineError::BadParams { .. })), "{cmd} accepted {p}");
        }
    }
    for (cmd, key) in [
        ("file.importVpkg", "dir"),
        ("file.importVpkg", "mode"),
        ("file.importVpkg", "lenient"),
        ("file.exportVpkg", "sequence"),
        ("file.exportVpkg", "id"),
        ("file.exportVpkg", "version"),
        ("file.exportVpkg", "title"),
    ] {
        for v in [json!(null), json!([]), json!({}), json!("x".repeat(17000))] {
            let mut p = json!({"path":"/virtual/a.zip"});
            p[key] = v;
            assert!(matches!(s.execute(cmd, p), Err(EngineError::BadParams { .. })), "{cmd} {key}");
        }
    }
    for dir in ["../out", "/virtual/../out", "..\\out"] {
        assert!(matches!(s.execute("file.importVpkg", json!({"path":"/virtual/a.zip","dir":dir})), Err(EngineError::BadParams { .. })));
    }
    *files.oversized.lock().unwrap() = true;
    assert!(matches!(s.execute("file.importVpkg", json!({"path":"/virtual/huge.zip"})), Err(EngineError::BadParams { .. })));
}

#[test]
fn vpkg_archive_traversal_writes_nothing() {
    let (mut s, files) = setup();
    let mut bytes = package();
    // ZIP names occur in local and central headers; changing both leaves payload CRCs intact.
    for i in 0..bytes.len().saturating_sub(8) {
        if &bytes[i..i + 8] == b"project/" {
            bytes[i..i + 8].copy_from_slice(b"../xxxx/");
        }
    }
    files.write_file("/virtual/bad.zip", &bytes).unwrap();
    let before = s.project.clone();
    let writes = *files.writes.lock().unwrap();
    let e = s.execute("file.importVpkg", json!({"path":"/virtual/bad.zip"})).unwrap_err();
    assert!(e.to_json()["diagnostics"].is_array());
    assert!(Arc::ptr_eq(&before, &s.project));
    assert_eq!(*files.writes.lock().unwrap(), writes);
}

#[cfg(unix)]
#[test]
fn vpkg_filesystem_symlink_escape() {
    let root = crate::media_test_util::tmp_dir("vpkg-containment");
    let dest = root.join("out");
    let outside = root.join("outside");
    std::fs::create_dir_all(dest.join("project")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, dest.join("project/assets")).unwrap();
    let archive = root.join("demo.zip");
    std::fs::write(&archive, package()).unwrap();
    let mut s = Session::default();
    let before = s.project.clone();
    assert!(s.execute("file.importVpkg", json!({"path":archive,"dir":dest})).is_err());
    assert!(Arc::ptr_eq(&before, &s.project));
    assert!(!outside.join("pixel.png").exists());
    assert!(!dest.join("vpkg.json").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn vpkg_explicit_directory_allows_identical_reimport() {
    let (mut s, files) = setup();
    files.write_file("/virtual/demo.zip", &package()).unwrap();
    for mode in ["native", "hybrid", "rendered"] {
        let before = s.history.undo.len();
        let r = s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip","dir":"/virtual/chosen","mode":mode})).unwrap();
        assert_eq!(r["package"]["extractedTo"], "/virtual/chosen");
        assert_eq!(s.history.undo.len(), before + 1);
    }
}

#[test]
fn vpkg_python_oracle() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    fn run(command: &mut Command) -> std::io::Result<bool> {
        let mut child = command.stdin(Stdio::null()).spawn()?;
        let start = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status.success());
            }
            if start.elapsed() >= Duration::from_secs(120) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "oracle exceeded 120 seconds"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let Some(core) = std::env::var_os("SR_CORE_DIR") else {
        eprintln!("skipped: SR_CORE_DIR is not set");
        return;
    };
    if !run(Command::new("python3").args(["-c", "import lxml"])).unwrap_or(false) {
        eprintln!("skipped: python3/lxml unavailable");
        return;
    }
    let (mut s, files) = setup();
    load(&mut s, &files);
    s.execute("file.exportVpkg", json!({"path":"/virtual/oracle.vpkg.zip"})).unwrap();
    let root = crate::media_test_util::tmp_dir("vpkg-python");
    let path = root.join("export.vpkg.zip");
    std::fs::write(&path, files.read_file("/virtual/oracle.vpkg.zip").unwrap()).unwrap();
    let result = run(Command::new("python3").args(["-m", "sr_core.vpkg", "verify"]).arg(path).env("PYTHONPATH", core));
    std::fs::remove_dir_all(root).unwrap();
    assert!(result.unwrap(), "sr-core rejected engine export");
}

#[test]
fn vpkg_export_defaults_and_duplicate_names() {
    let (mut s, files) = setup();
    let xml = scene()
        .replace("assets/pixel.png", "/one/pixel.png")
        .replace("</assets>", r#"<image id="other" src="/two/pixel.png" width="1" height="1"/></assets>"#)
        .replace("</composition>", r#"<layer id="second" asset="other" start="0" end="1"/></composition>"#);
    let mut second = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(1, 1, image::Rgba([201, 17, 3, 255])).write_to(&mut second, image::ImageFormat::Png).unwrap();
    files.write_file("/one/pixel.png", &png()).unwrap();
    files.write_file("/two/pixel.png", second.get_ref()).unwrap();
    files.write_file("/virtual/Fancy Name.scene", xml.as_bytes()).unwrap();
    s.execute("file.importScene", json!({"path":"/virtual/Fancy Name.scene","mode":"native"})).unwrap();
    s.execute("file.exportVpkg", json!({"path":"/virtual/export.zip"})).unwrap();
    let bytes = files.read_file("/virtual/export.zip").unwrap();
    let p = filmcraft_vpkg::read(&bytes, &Limits::default()).unwrap();
    assert_eq!(p.manifest.data.package.id, "fancy-name");
    assert_eq!(p.manifest.data.package.title, "Fancy Name");
    assert_eq!(p.manifest.data.package.version, "1.0.0");
    let assets: Vec<_> = p.entries.iter().filter(|e| e.path.starts_with("project/assets/")).collect();
    assert_eq!(assets.len(), 2);
    assert_ne!(assets[0].path, assets[1].path);
    assert!(assets.iter().any(|e| e.bytes == png()));
    assert!(assets.iter().any(|e| &e.bytes == second.get_ref()));
}

#[test]
fn vpkg_native_lenient_matches_scene() {
    let (mut s, files) = setup();
    let bytes = filmcraft_vpkg::pack(&PackInput {
        primary: FileInput { path: "main.scene".into(), bytes: br#"<scene version="1.7"><project width="32" height="24" fps="24" duration="1"/><composition><particleEmitter id="particles" start="0" end="1"/></composition></scene>"#.to_vec(), role: "scene".into() },
        files: vec![], metadata: Metadata { id: "demo".into(), version: "1.0.0".into(), title: "Demo".into(), created: None }, limits: Limits::default(), source_date_epoch: None,
    }).unwrap();
    files.write_file("/virtual/demo.zip", &bytes).unwrap();
    let before = s.project.clone();
    let e = s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip","dir":"/virtual/out","mode":"native"})).unwrap_err();
    assert!(e.to_string().contains("1 scene overlay"));
    assert!(Arc::ptr_eq(&before, &s.project));
    let r = s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip","dir":"/virtual/out","mode":"native","lenient":true})).unwrap();
    assert!(r["report"].as_array().unwrap().iter().any(|v| v.as_str().unwrap().contains("dropped")));
}

#[test]
fn vpkg_filesystem_import() {
    let root = crate::media_test_util::tmp_dir("vpkg-extract");
    let archive = root.join("demo.vpkg.zip");
    std::fs::write(&archive, package()).unwrap();
    let mut s = Session::default();
    let before = s.project.clone();
    let r = s.execute("file.importVpkg", json!({"path":archive,"mode":"native"})).unwrap();
    assert_eq!(count(&s, &r), 2);
    assert_eq!(std::fs::read(root.join("demo/project/assets/pixel.png")).unwrap(), png());
    s.execute("edit.undo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&before, &s.project));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn vpkg_extraction_rejects_device_aliases() {
    let (_, files) = setup();
    for name in ["CON", "nul.txt", "aux", "PRN", "com1.wav", "LPT9", "COM¹", "CONIN$", "CONOUT$"] {
        assert!(files.package_path("/virtual/out", &format!("project/{name}")).is_err(), "accepted device alias {name}");
    }
}

fn custom_package(xml: &str, asset: Option<(&str, Vec<u8>)>) -> Vec<u8> {
    filmcraft_vpkg::pack(&PackInput {
        primary: FileInput { path: "main.scene".into(), bytes: xml.as_bytes().into(), role: "scene".into() },
        files: asset.into_iter().map(|(path, bytes)| FileInput { path: path.into(), bytes, role: "asset".into() }).collect(),
        metadata: Metadata { id: "demo".into(), version: "1.0.0".into(), title: "Demo".into(), created: None },
        limits: Limits::default(),
        source_date_epoch: None,
    })
    .unwrap()
}

#[cfg(unix)]
#[test]
fn vpkg_fix_overlay_symlink_preflight() {
    let root = crate::media_test_util::tmp_dir("vpkg-overlay-link");
    let dest = root.join("out");
    let outside = root.join("outside");
    std::fs::create_dir_all(dest.join("project")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, dest.join("project/main Scene Overlays")).unwrap();
    let xml = r#"<scene version="1.7"><project width="32" height="24" fps="24" duration="1"/><composition><particleEmitter id="particles" start="0" end="1"/></composition></scene>"#;
    let archive = root.join("demo.zip");
    std::fs::write(&archive, custom_package(xml, None)).unwrap();
    let mut s = Session::default();
    let before = s.project.clone();
    let result = s.execute("file.importVpkg", json!({"path":archive,"dir":dest}));
    let outside_files = std::fs::read_dir(&outside).unwrap().count();
    let extracted = dest.join("vpkg.json").exists();
    std::fs::remove_dir_all(root).unwrap();
    assert!(result.is_err(), "hybrid import followed an overlay symlink: {result:?}");
    assert_eq!(outside_files, 0);
    assert!(!extracted, "overlay preflight must precede extraction");
    assert!(Arc::ptr_eq(&before, &s.project));
}

#[test]
fn vpkg_fix_uri_escape_and_suffix() {
    // refs.py _value_path: strip query/fragment, then percent-decode once.
    // Neither the literal percent filename nor the apparent query traversal is an input.
    for (uri, name) in [
        ("assets/a%20b.png", "assets/a b.png"),
        ("assets/a%2520b.png", "assets/a%20b.png"),
        ("assets/pixel.png?x/../../../../outside.png", "assets/pixel.png"),
        ("assets/pixel.png#frag", "assets/pixel.png"),
    ] {
        for mode in ["native", "hybrid", "rendered", "overlay"] {
            let (mut s, files) = setup();
            let mut xml = scene().replace("assets/pixel.png", uri);
            if mode == "overlay" {
                xml = xml.replace("<composition>", "<composition><group id=\"fx\" threeD=\"true\">").replace("</composition>", "</group></composition>");
            }
            files.write_file("/virtual/demo.zip", &custom_package(&xml, Some((name, png())))).unwrap();
            let r = s
                .execute("file.importVpkg", json!({"path":"/virtual/demo.zip","dir":"/virtual/out","mode":if mode == "overlay" {"hybrid"} else {mode}}))
                .unwrap();
            assert_eq!(files.read_file("/virtual/out/project/main.scene").unwrap(), xml.as_bytes(), "verified bytes are preserved");
            let media_paths: Vec<_> = s
                .project
                .items
                .values()
                .filter_map(|i| match &i.as_media()?.media {
                    filmcraft_project::MediaRef::File { path } => Some(path.clone()),
                    _ => None,
                })
                .collect();
            let expected = format!("/virtual/out/project/{name}");
            if matches!(mode, "rendered" | "overlay") {
                let scenes: Vec<_> = media_paths.iter().filter(|path| path.ends_with(".scene")).collect();
                assert_eq!(scenes.len(), 1);
                assert!(media_paths.iter().filter(|path| !path.ends_with(".scene")).all(|path| *path == expected));
                let bytes = files.read_file(scenes[0]).unwrap();
                let text = std::str::from_utf8(&bytes).unwrap();
                let doc = filmcraft_interchange::scene::read::read(text, &Default::default()).unwrap();
                let src = doc.doc.descendants().find(|n| n.has_tag_name("image")).unwrap().attribute("src").unwrap();
                assert_eq!(filmcraft_interchange::file_url_to_path(src), expected, "{mode}: {uri}");
            } else {
                assert_eq!(r["linkedMedia"], 1, "{mode}: {uri}: {r}");
                assert_eq!(media_paths, [expected]);
            }
        }
    }
}

#[test]
fn vpkg_fix_query_never_opens_external_file() {
    let (mut s, files) = setup();
    let xml = scene().replace("assets/pixel.png", "assets/pixel.png?x/../../../../outside.png");
    files.write_file("/virtual/demo.zip", &custom_package(&xml, Some(("assets/pixel.png", png())))).unwrap();
    files.write_file("/virtual/outside.png", b"unverified").unwrap();
    let _ = s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip","dir":"/virtual/out","mode":"native"}));
    assert!(!files.reads.lock().unwrap().iter().any(|p| p == "/virtual/outside.png"), "opened an unverified external file");
}

#[test]
fn vpkg_fix_sequence_pattern_preserved() {
    // Image sequence %02d and ## are patterns, not escapes/fragments (refs.py).
    for pattern in ["frame_%02d.png", "frame_##.png"] {
        let (mut s, files) = setup();
        let xml = scene().replace("<image ", "<imageSequence first=\"1\" last=\"1\" fps=\"1\" ").replace("assets/pixel.png", &format!("assets/{pattern}"));
        files.write_file("/virtual/demo.zip", &custom_package(&xml, Some(("assets/frame_01.png", png())))).unwrap();
        let r = s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip","mode":"native","lenient":true})).unwrap();
        assert_eq!(count(&s, &r), 2);
        assert!(
            s.project
                .items
                .values()
                .any(|i| matches!(i.as_media().map(|m| &m.media), Some(filmcraft_project::MediaRef::File { path }) if path.ends_with(pattern)))
        );
    }
}

#[test]
fn vpkg_fix_rendered_ingest_one_undo() {
    let root = crate::media_test_util::tmp_dir("vpkg-ingest-undo");
    let archive = root.join("demo.zip");
    let dest = root.join("ingest");
    std::fs::write(&archive, package()).unwrap();
    let mut s = Session::default();
    s.execute("project.ingestSettings", json!({"enabled":true,"action":"copy","destination":dest})).unwrap();
    let before = s.project.clone();
    let undo = s.history.undo.len();
    let r = s.execute("file.importVpkg", json!({"path":archive,"mode":"rendered"})).unwrap();
    assert_eq!(r["ingest"]["copied"].as_array().unwrap().len(), 1, "ingest remains enabled");
    let steps = s.history.undo.len() - undo;
    s.execute("edit.undo", json!({})).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(steps, 1, "import and synchronous ingest must share one undo step");
    assert!(Arc::ptr_eq(&before, &s.project));
}

#[test]
fn vpkg_fix_failed_extraction_recovery_context() {
    let (mut s, files) = setup();
    files.write_file("/virtual/demo.zip", &package()).unwrap();
    *files.fail_after.lock().unwrap() = Some(2); // README follows the two metadata entries.
    let before = s.project.clone();
    let err = s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip","dir":"/virtual/out"})).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("/virtual/out") && message.contains("retained") && message.contains("vpkg.json") && message.contains("vpkg.schema.json"),
        "{message}"
    );
    assert!(Arc::ptr_eq(&before, &s.project));
    *files.fail_after.lock().unwrap() = None;
    s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip","dir":"/virtual/out"})).unwrap();
}

#[test]
fn vpkg_fix_partial_native_write_is_removed() {
    use std::io::Write;
    let root = crate::media_test_util::tmp_dir("vpkg-partial-write");
    let path = root.join("partial.bin");
    let result = crate::write_new_package_file(path.to_str().unwrap(), b"complete data", |file, bytes| {
        file.write_all(&bytes[..3])?;
        Err(std::io::Error::other("injected disk full"))
    });
    let remains = path.exists();
    std::fs::remove_dir_all(root).unwrap();
    assert!(result.unwrap_err().to_string().contains("disk full"));
    assert!(!remains, "failed exclusive write left a truncated file");
}

#[test]
fn vpkg_round2_failed_write_preserves_replacement() {
    use std::io::Write;
    let root = crate::media_test_util::tmp_dir("vpkg-replacement");
    let dest = root.join("asset.bin");
    let result = crate::write_new_package_file(dest.to_str().unwrap(), b"package", |file, bytes| {
        file.write_all(&bytes[..2])?;
        // Deterministic scheduling of a concurrent replacement before a write fails.
        if dest.exists() {
            std::fs::rename(&dest, root.join("displaced.bin"))?;
        }
        std::fs::write(&dest, b"valuable")?;
        Err(std::io::Error::other("injected disk full"))
    });
    let retained = std::fs::read(&dest).ok();
    std::fs::remove_dir_all(root).unwrap();
    assert!(result.is_err());
    assert_eq!(retained.as_deref(), Some(b"valuable".as_slice()), "cleanup deleted a replacement destination");
}

#[test]
fn vpkg_round2_publish_never_clobbers() {
    use std::io::Write;
    let root = crate::media_test_util::tmp_dir("vpkg-publish");
    let dest = root.join("asset.bin");
    let result = crate::write_new_package_file(dest.to_str().unwrap(), b"package", |file, bytes| {
        if dest.exists() {
            std::fs::rename(&dest, root.join("displaced.bin"))?;
        }
        std::fs::write(&dest, b"valuable")?;
        file.write_all(bytes)
    });
    let retained = std::fs::read(&dest).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(result.is_err(), "publication must report the occupied destination");
    assert_eq!(retained, b"valuable");
}

#[test]
fn vpkg_round2_windows_path_comparison() {
    for (root, target, expected) in [
        (r"\\?\C:\out", "C:/out/project/assets/pixel.png", "project/assets/pixel.png"),
        (r"C:\out", r"\\?\c:\OUT\project\a.png", "project/a.png"),
        (r"\\?\UNC\server\share\out", "//server/share/out/project/a.png", "project/a.png"),
        ("/tmp/out", "/tmp/out/project/a.png", "project/a.png"),
    ] {
        assert_eq!(crate::vpkg::relative_path(root, target).as_deref(), Some(expected));
    }
    for (root, target) in [(r"\\?\C:\out", "C:/outside/a.png"), (r"C:\out", "D:/out/a.png"), ("/tmp/out", "/tmp/OUT/a.png")] {
        assert!(crate::vpkg::relative_path(root, target).is_none());
    }
}

fn particle_scene() -> &'static str {
    r#"<scene version="1.7"><project width="32" height="24" fps="24" duration="1"/><composition><particleEmitter id="particles" start="0" end="1"/></composition></scene>"#
}

#[test]
fn vpkg_round2_generated_prefix_conflicts_preflight() {
    for name in ["main Scene Overlays", "MAIN SCENE OVERLAYS", "main Scene Overlays/0-PARTICLES.SCENE"] {
        let (mut s, files) = setup();
        files.write_file("/virtual/demo.zip", &custom_package(particle_scene(), Some((name, b"occupied".to_vec())))).unwrap();
        let before = s.project.clone();
        let writes = *files.writes.lock().unwrap();
        let result = s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip","dir":"/virtual/out"}));
        // A packaged file cannot also be the parent of a generated overlay.
        assert!(result.is_err(), "accepted generated file/directory collision: {name}");
        assert_eq!(*files.writes.lock().unwrap(), writes, "predictable conflict left extracted files");
        assert!(Arc::ptr_eq(&before, &s.project));
    }
}

fn with_fetch(bytes: &[u8], missing: &str) -> Vec<u8> {
    let mut entries = filmcraft_vpkg::zip::read(bytes, &Limits::default()).unwrap();
    let manifest = entries.iter_mut().find(|e| e.path == "vpkg.json").unwrap();
    let mut doc: Value = serde_json::from_slice(&manifest.bytes).unwrap();
    let files = doc["files"].as_array_mut().unwrap();
    let index = files.iter().position(|f| f["path"] == missing).unwrap();
    let file = files.remove(index);
    doc["fetch"] = json!([{"path":missing,"url":"https://example.org/input","sha256":file["sha256"],"size":file["size"]}]);
    manifest.bytes = serde_json::to_vec(&doc).unwrap();
    entries.retain(|e| e.path != format!("project/{missing}"));
    let inputs: Vec<_> = entries.iter().map(|e| filmcraft_vpkg::zip::Input { path: &e.path, bytes: &e.bytes, executable: false }).collect();
    filmcraft_vpkg::zip::write(&inputs, &Limits::default(), None).unwrap()
}

fn include_package(part: &str, dependencies: Vec<FileInput>) -> Vec<u8> {
    let xml = r#"<scene version="1.7"><project width="32" height="24" fps="24" duration="1"/><include src="parts/part.scene"/><composition/></scene>"#;
    let mut files = vec![FileInput { path: "parts/part.scene".into(), bytes: part.as_bytes().to_vec(), role: "include".into() }];
    files.extend(dependencies);
    filmcraft_vpkg::pack(&PackInput {
        primary: FileInput { path: "main.scene".into(), bytes: xml.as_bytes().to_vec(), role: "scene".into() },
        files,
        metadata: Metadata { id: "demo".into(), version: "1.0.0".into(), title: "Demo".into(), created: None },
        limits: Limits::default(),
        source_date_epoch: None,
    })
    .unwrap()
}

#[cfg(unix)]
#[test]
fn vpkg_round2_included_fetch_symlink_preflight() {
    let part = r#"<assets><image id="i" src="generated/pixel.png"/></assets>"#;
    let bytes = include_package(part, vec![FileInput { path: "parts/generated/pixel.png".into(), bytes: png(), role: "asset".into() }]);
    let bytes = with_fetch(&bytes, "parts/generated/pixel.png");
    assert!(filmcraft_vpkg::read(&bytes, &Limits::default()).is_ok());
    let root = crate::media_test_util::tmp_dir("vpkg-include-link");
    let dest = root.join("out");
    let outside = root.join("outside");
    std::fs::create_dir_all(dest.join("project/parts")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("pixel.png"), png()).unwrap();
    std::os::unix::fs::symlink(&outside, dest.join("project/parts/generated")).unwrap();
    let archive = root.join("demo.zip");
    std::fs::write(&archive, bytes).unwrap();
    let mut s = Session::default();
    let before = s.project.clone();
    let result = s.execute("file.importVpkg", json!({"path":archive,"dir":dest,"mode":"rendered"}));
    let extracted = dest.join("vpkg.json").exists();
    std::fs::remove_dir_all(root).unwrap();
    assert!(result.is_err(), "included input escaped containment");
    assert!(!extracted, "dependency graph must be checked before extraction");
    assert!(Arc::ptr_eq(&before, &s.project));
}

#[test]
fn vpkg_round2_included_references_normalized() {
    let bytes = include_package(
        r#"<assets><image id="i" src="a%20b.png?ignored"/></assets>"#,
        vec![FileInput { path: "parts/a b.png".into(), bytes: png(), role: "asset".into() }],
    );
    let (mut s, files) = setup();
    files.write_file("/virtual/demo.zip", &bytes).unwrap();
    s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip","dir":"/virtual/out","mode":"rendered"})).unwrap();
    let generated = files.read_file("/virtual/out/project/parts/part.scene.filmcraft.scene").unwrap();
    let text = std::str::from_utf8(&generated).unwrap();
    assert!(text.contains("file:///virtual/out/project/parts/a%20b.png"), "{text}");
    assert!(!text.contains("?ignored"));
}

#[test]
fn vpkg_round2_renderer_inputs_cannot_escape() {
    for node in [
        r#"<tiles id="t" src="../../outside.pmtiles"/>"#,
        r#"<tiles id="t" cache="../../outside.pmtiles"/>"#,
        r#"<volume id="v" src="../../outside.srvol"/>"#,
        r#"<basemap tiles="t" mapStyle="../../outside.json"/>"#,
        r#"<strokeFont id="f" src="../../outside.jhf"/>"#,
        r#"<geo id="g" src="../../outside.geojson"/>"#,
        r#"<pdf id="p" cache="../../outside.png"/>"#,
        r#"<compute src="../../outside.wgsl"/>"#,
        r#"<voxelAsset id="v" src="../../outside.vox"/>"#,
        r#"<meshSequence id="m" src="../../outside_%02d.obj"/>"#,
        r#"<program src="../../outside.wasm"/>"#,
    ] {
        let (mut s, files) = setup();
        let xml = particle_scene().replace("<composition>", &format!("<assets>{node}</assets><composition>"));
        let bytes = custom_package(&xml, None);
        files.write_file("/virtual/demo.zip", &bytes).unwrap();
        let writes = *files.writes.lock().unwrap();
        let result = s.execute("file.importVpkg", json!({"path":"/virtual/demo.zip","dir":"/virtual/out","mode":"rendered"}));
        assert!(result.is_err(), "unclassified renderer input: {node}");
        assert_eq!(*files.writes.lock().unwrap(), writes, "wrote before checking {node}");
    }
}

#[test]
fn vpkg_round2_xsd_file_attribute_parity() {
    // Wrapping lets the bounded XML reader inspect XSD without a new dependency.
    let schema = include_str!("../../interchange/schema/sr-core/scene-render.xsd");
    let body = schema.strip_prefix("<?xml").map_or(schema, |_| schema.split_once("?>").unwrap().1);
    let wrapped = format!("<scene version=\"1.7\">{body}</scene>");
    let doc = filmcraft_interchange::scene::read::read(&wrapped, &Default::default()).unwrap();
    let nodes: Vec<_> = doc.doc.descendants().filter(|n| n.is_element()).collect();
    for attr in nodes.iter().filter(|n| n.tag_name().name() == "attribute") {
        let name = attr.attribute("name").unwrap_or("");
        let docs = attr.descendants().filter(|n| n.is_text()).filter_map(|n| n.text()).collect::<String>().to_lowercase();
        let file = matches!(attr.attribute("type"), Some("xs:anyURI" | "volumeSourceType"))
            || (attr.attribute("type") == Some("xs:string")
                && docs.contains("file")
                && (docs.contains("path") || docs.contains("pattern") || docs.contains("model file")));
        if !file {
            continue;
        }
        let owner = attr.ancestors().skip(1).find(|n| matches!(n.tag_name().name(), "complexType" | "attributeGroup" | "element")).unwrap();
        let mut owners = std::collections::BTreeSet::new();
        if let Some(name) = owner.attribute("name") {
            owners.insert(name.to_string());
        }
        // Follow attribute groups and type inheritance, including inline element types.
        for _ in 0..nodes.len() {
            let before = owners.len();
            for node in &nodes {
                if node.attribute("ref").or_else(|| node.attribute("base")).is_some_and(|r| owners.contains(r)) {
                    if let Some(parent) = node.ancestors().skip(1).find(|n| matches!(n.tag_name().name(), "complexType" | "attributeGroup" | "element")) {
                        if let Some(name) = parent.attribute("name") {
                            owners.insert(name.to_string());
                        } else if let Some(element) = parent.parent().filter(|n| n.tag_name().name() == "element") {
                            owners.insert(element.attribute("name").unwrap().to_string());
                        }
                    }
                }
            }
            if owners.len() == before {
                break;
            }
        }
        let tags: std::collections::BTreeSet<_> = nodes
            .iter()
            .filter(|n| n.tag_name().name() == "element")
            .filter(|n| {
                n.attribute("type").is_some_and(|t| owners.contains(t))
                    || n.attribute("name").is_some_and(|t| owners.contains(t))
                    || n.children().any(|c| c == owner)
            })
            .filter_map(|n| n.attribute("name"))
            .collect();
        assert!(!tags.is_empty(), "unresolved XSD attribute owner: {owners:?}@{name}");
        for tag in tags {
            if matches!(
                (tag, name),
                ("output", "path" | "report") | ("poster" | "thumbnail" | "still", "path") | ("destination", "uri") | ("captionTrack", "model")
            ) {
                continue;
            }
            assert!(crate::vpkg::input_attribute(tag, name), "unclassified XSD file input: {tag}@{name}");
        }
    }
}

#[cfg(unix)]
#[test]
fn vpkg_round2_gltf_dependency_symlink_preflight() {
    let part = r#"<assets><mesh id="m" src="model.gltf"/></assets>"#;
    let bytes = include_package(
        part,
        vec![
            FileInput {
                path: "parts/model.gltf".into(),
                bytes: br#"{"asset":{"version":"2.0"},"buffers":[{"uri":"generated/buffer.bin","byteLength":4}]}"#.to_vec(),
                role: "asset".into(),
            },
            FileInput { path: "parts/generated/buffer.bin".into(), bytes: vec![0; 4], role: "asset".into() },
        ],
    );
    let bytes = with_fetch(&bytes, "parts/generated/buffer.bin");
    assert!(filmcraft_vpkg::read(&bytes, &Limits::default()).is_ok());
    let root = crate::media_test_util::tmp_dir("vpkg-gltf-link");
    let dest = root.join("out");
    let outside = root.join("outside");
    std::fs::create_dir_all(dest.join("project/parts")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, dest.join("project/parts/generated")).unwrap();
    let archive = root.join("demo.zip");
    std::fs::write(&archive, bytes).unwrap();
    let mut s = Session::default();
    let result = s.execute("file.importVpkg", json!({"path":archive,"dir":dest,"mode":"rendered"}));
    let extracted = dest.join("vpkg.json").exists();
    std::fs::remove_dir_all(root).unwrap();
    assert!(result.is_err(), "glTF dependency escaped containment");
    assert!(!extracted, "dependency check happened after extraction");
}
