#![cfg(test)]
use filmcraft_vpkg::{FileInput, Limits, PackInput, manifest::Metadata, pack, read, zip};
use proptest::prelude::*;
use serde_json::{Value, json};
const SCENE:&[u8]=br#"<scene version="1.7"><project width="320" height="180" fps="30000/1001" duration="2.5"/><assets><data id="d" src="data.json"/></assets><composition/></scene>"#;
fn input() -> PackInput {
    PackInput {
        primary: FileInput { path: "main.scene".into(), bytes: SCENE.to_vec(), role: "scene".into() },
        files: vec![FileInput { path: "data.json".into(), bytes: b"{}".to_vec(), role: "data".into() }],
        metadata: Metadata { id: "demo".into(), version: "1.0.0".into(), title: "Demo".into(), created: Some("2026-10-14".into()) },
        limits: Limits::default(),
        source_date_epoch: None,
    }
}
fn rewrite(bytes: &[u8], f: impl FnOnce(&mut Vec<zip::Entry>, &mut Value)) -> Vec<u8> {
    let mut entries = zip::read(bytes, &Limits::default()).unwrap();
    let mut m: Value = serde_json::from_slice(&entries[0].bytes).unwrap();
    f(&mut entries, &mut m);
    entries.iter_mut().find(|e| e.path == "vpkg.json").unwrap().bytes = serde_json::to_vec(&m).unwrap();
    let inputs: Vec<_> = entries.iter().map(|e| zip::Input { path: &e.path, bytes: &e.bytes, executable: e.executable }).collect();
    zip::write(&inputs, &Limits::default(), None).unwrap()
}
const ORACLE: &[u8] = include_bytes!("fixtures/minimal.vpkg.zip");
#[test]
fn python_oracles() {
    for bytes in [ORACLE, include_bytes!("fixtures/includes.vpkg.zip"), include_bytes!("fixtures/fetch.vpkg.zip")] {
        let p = read(bytes, &Limits::default()).unwrap();
        assert_eq!(p.primary_scene, "main.scene");
    }
}
#[test]
fn package_writer_facts_order_hashes_and_determinism() {
    let i = input();
    let b = pack(&i).unwrap();
    assert_eq!(b, pack(&i).unwrap());
    let p = read(&b, &Limits::default()).unwrap();
    assert_eq!(p.manifest.data.format_version, "1.1");
    assert_eq!(p.primary_scene, "main.scene");
    let m = &p.manifest.raw;
    // Facts written by hand: 320×180 at 30000/1001 for 2.5 seconds; no audio or captions.
    assert_eq!(m["video"]["width"], 320);
    assert_eq!(m["video"]["height"], 180);
    assert_eq!(m["video"]["fps"], "30000/1001");
    assert_eq!(m["video"]["duration"], 2.5);
    assert_eq!(m["video"]["audio"], false);
    assert_eq!(m["video"]["captions"], json!([]));
    assert_eq!(m["packed"]["tool"], "filmcraft");
    assert_eq!(m["packed"]["fileCount"], 2);
    assert_eq!(m["packed"]["totalSize"], SCENE.len() + 2);
    let names: Vec<_> = p.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, ["vpkg.json", "vpkg.schema.json", "README.md", "project/data.json", "project/main.scene"]);
    assert_eq!(p.entries.iter().find(|e| e.path == "project/main.scene").unwrap().bytes, SCENE);
}
#[test]
fn package_stray_hash_first_and_metadata_failures() {
    for mode in 0..8 {
        let b = rewrite(ORACLE, |e, m| match mode {
            0 => e.push(zip::Entry { path: "stray".into(), bytes: vec![], executable: false }),
            1 => m["files"][0]["sha256"] = json!("0".repeat(64)),
            2 => e.swap(0, 1),
            3 => m["packed"]["metaSha256"]["README.md"] = json!("0".repeat(64)),
            4 => {
                m["packed"].as_object_mut().unwrap().remove("metaSha256");
            }
            5 => {
                e.retain(|e| e.path != "README.md");
            }
            6 => m["files"][0]["size"] = json!(1),
            _ => {
                e.retain(|e| !e.path.starts_with("project/"));
            }
        });
        assert!(read(&b, &Limits::default()).is_err(), "mode {mode}");
    }
}
#[test]
fn package_legacy_and_future_minor() {
    let b = rewrite(ORACLE, |_, m| {
        m["formatVersion"] = json!("1.0");
        m["packed"].as_object_mut().unwrap().remove("metaSha256");
    });
    let p = read(&b, &Limits::default()).unwrap();
    assert!(p.warnings.iter().any(|w| w.code == "META_UNHASHED"));
    let b = rewrite(ORACLE, |_, m| {
        m["formatVersion"] = json!("1.42");
        m["extension"] = json!({"keep":true});
    });
    let p = read(&b, &Limits::default()).unwrap();
    assert_eq!(p.manifest.raw["extension"]["keep"], true);
    assert!(!p.warnings.is_empty());
}
#[test]
fn writer_rejects_missing_remote_unsafe_duplicate_and_bad_numbers() {
    let mut i = input();
    i.files.clear();
    assert!(pack(&i).is_err());
    for path in ["../main.scene", "/main.scene", "C:x"] {
        let mut i = input();
        i.primary.path = path.into();
        assert!(pack(&i).is_err());
    }
    let mut i = input();
    i.files.push(i.files[0].clone());
    assert!(pack(&i).is_err());
    let mut i = input();
    i.primary.bytes = String::from_utf8(i.primary.bytes).unwrap().replace("data.json", "https://example.org/data.json").into_bytes();
    assert!(pack(&i).is_err());
    for (from, to) in [("2.5", "NaN"), ("2.5", "inf"), ("320", "-1"), ("30000/1001", "1/0")] {
        let mut i = input();
        i.primary.bytes = String::from_utf8(i.primary.bytes).unwrap().replace(from, to).into_bytes();
        assert!(pack(&i).is_err());
    }
}
#[test]
fn reference_closure_missing_input_and_output_classification() {
    let mut i = input();
    i.primary.bytes=br#"<scene version="1.7"><project width="1" height="1" fps="1" duration="1"/><assets><data src="data.json"/></assets><outputs><output id="main" path="absent.mp4"/><poster path="absent.png"/><thumbnail path="absent.png"/><still path="absent.png"/><destination uri="https://example.org/out"/></outputs><composition/></scene>"#.to_vec();
    assert!(pack(&i).is_ok());
    let b = pack(&input()).unwrap();
    let b = rewrite(&b, |e, m| {
        e.retain(|e| e.path != "project/data.json");
        m["files"].as_array_mut().unwrap().retain(|f| f["path"] != "data.json");
    });
    assert!(read(&b, &Limits::default()).is_err());
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn generated_small_packages_round_trip(data in proptest::collection::vec(any::<u8>(),0..2048), extra in proptest::collection::vec(any::<u8>(),0..256)) {
        let mut i=input();i.files[0].bytes=data.clone();i.files.push(FileInput{path:"other.bin".into(),bytes:extra.clone(),role:"asset".into()});
        let b=pack(&i).unwrap();let p=read(&b,&i.limits).unwrap();
        prop_assert_eq!(&p.entries.iter().find(|e|e.path=="project/data.json").unwrap().bytes,&data);
        prop_assert_eq!(&p.entries.iter().find(|e|e.path=="project/other.bin").unwrap().bytes,&extra);
    }
    #[test]
    fn mutated_packages_never_panic(index in 0usize..10000, value in any::<u8>()) {
        let mut b=ORACLE.to_vec();let idx=index%b.len();b[idx]^=value;
        prop_assert!(std::panic::catch_unwind(||read(&b,&Limits::default())).is_ok());
    }
}

fn scene_with(body: &str) -> Vec<u8> {
    format!(r#"<scene version="1.7"><project width="16" height="16" fps="24" duration="1"/>{body}<composition/></scene>"#).into_bytes()
}
#[test]
fn references_all_input_attributes_and_optional_caches() {
    for (tag, attr) in [
        ("audio", "src"),
        ("captionTrack", "src"),
        ("chart", "src"),
        ("data", "src"),
        ("effect", "src"),
        ("font", "src"),
        ("image", "src"),
        ("imageSequence", "src"),
        ("look", "src"),
        ("lottie", "src"),
        ("mesh", "src"),
        ("representation", "src"),
        ("trackData", "src"),
        ("vector", "src"),
        ("video", "src"),
        ("audio", "proxy"),
        ("font", "proxy"),
        ("image", "proxy"),
        ("imageSequence", "proxy"),
        ("lottie", "proxy"),
        ("mesh", "proxy"),
        ("vector", "proxy"),
        ("video", "proxy"),
        ("span", "fontFile"),
        ("text", "fontFile"),
        ("textStyle", "fontFile"),
        ("light", "environment"),
        ("light", "ies"),
        ("colorManagement", "ocioConfig"),
        ("transition", "shader"),
        ("skeleton", "weights"),
        ("material", "baseColorMap"),
        ("material", "displacementMap"),
        ("material", "emissiveMap"),
        ("material", "materialX"),
        ("material", "metallicRoughnessMap"),
        ("material", "normalMap"),
        ("material", "occlusionMap"),
    ] {
        let mut i = input();
        i.primary.bytes = scene_with(&format!("<{tag} {attr}=\"data.json\"/>"));
        assert!(pack(&i).is_ok(), "{tag} {attr}");
        i.files.clear();
        assert!(pack(&i).is_err(), "{tag} {attr}");
    }
    let mut i = input();
    i.files.clear();
    i.primary.bytes = scene_with(r#"<physics cache="missing.bin"/><generated cache="missing.bin"/><captionTrack cache="missing.bin"/>"#);
    assert!(pack(&i).is_ok());
}
#[test]
fn references_includes_sequences_uri_and_nested_assets() {
    let mut i = input();
    i.primary.path = "scenes/main.scene".into();
    i.primary.bytes = scene_with(r#"<include src="../parts/inc.xml"/>"#);
    i.files = vec![
        FileInput {
            path: "parts/inc.xml".into(),
            bytes: br#"<?xml version="1.0"?><assets><data src="../data%20file.json#part"/><include src="inc.xml"/></assets>"#.to_vec(),
            role: "include".into(),
        },
        FileInput { path: "data file.json".into(), bytes: b"{}".to_vec(), role: "data".into() },
    ];
    assert!(pack(&i).is_ok());
    i.files.pop();
    assert!(pack(&i).is_err());
    for (pattern, good, bad) in [("frame_%02d.png", "frame_001.png", "frame_1.png"), ("frame_###.png", "frame_001.png", "frame_0001.png")] {
        let mut i = input();
        i.primary.bytes = scene_with(&format!("<imageSequence src=\"{pattern}\"/>"));
        i.files = vec![FileInput { path: good.into(), bytes: vec![], role: "asset".into() }];
        assert!(pack(&i).is_ok());
        i.files[0].path = bad.into();
        assert!(pack(&i).is_err());
    }
    for (path, bytes) in [
        ("mesh.gltf", br#"{"buffers":[{"uri":"data.json"}],"images":[{"uri":"data:application/octet-stream;base64,AA=="}]}"#.as_slice()),
        ("mesh.obj", b"mtllib data.json\n"),
        ("mesh.mtl", b"map_Kd data.json\n"),
        ("mesh.mtlx", br#"<materialx><input type="filename" value="data.json"/></materialx>"#),
    ] {
        let mut i = input();
        i.primary.bytes = scene_with(&format!("<mesh src=\"{path}\"/>"));
        i.files.push(FileInput { path: path.into(), bytes: bytes.to_vec(), role: "asset".into() });
        assert!(pack(&i).is_ok());
        i.files.remove(0);
        assert!(pack(&i).is_err());
    }
}
#[test]
fn references_escape_drafts_and_metadata_audio() {
    for reference in ["../outside.json", "%2e%2e/outside.json", "file:///tmp/outside.json", "a%00.json"] {
        let mut i = input();
        i.primary.bytes = scene_with(&format!("<data src=\"{reference}\"/>"));
        assert!(pack(&i).is_err());
    }
    let b = rewrite(ORACLE, |e, m| {
        let data = b"not even XML";
        e.push(zip::Entry { path: "project/draft.scene".into(), bytes: data.to_vec(), executable: false });
        m["scenes"].as_array_mut().unwrap().push(json!({"path":"draft.scene","role":"draft"}));
        m["files"].as_array_mut().unwrap().push(json!({"path":"draft.scene","size":data.len(),"sha256":filmcraft_bitstream::sha256::to_hex(&filmcraft_bitstream::sha256::sha256(data)),"role":"scene"}));
        m["packed"]["fileCount"] = json!(2);
        m["packed"]["totalSize"] = json!(112);
    });
    assert!(read(&b, &Limits::default()).is_ok());
    let mut i = input();
    i.primary.bytes = scene_with(r#"<audio src="data.json"/><captionTrack language="pt"/><captionTrack lang="en"/><captionTrack language="pt"/>"#);
    let p = read(&pack(&i).unwrap(), &i.limits).unwrap();
    assert_eq!(p.manifest.raw["video"]["audio"], true);
    assert_eq!(p.manifest.raw["video"]["captions"], json!(["en", "pt"]));
}

#[test]
fn includes_audio_contributes_video_metadata() {
    let mut i = input();
    i.primary.bytes = scene_with(r#"<include src="audio.xml"/>"#);
    i.files.push(FileInput { path: "audio.xml".into(), bytes: br#"<assets><audio src="data.json"/></assets>"#.to_vec(), role: "include".into() });
    let p = read(&pack(&i).unwrap(), &i.limits).unwrap();
    assert_eq!(p.manifest.raw["video"]["audio"], true);
}
#[test]
fn included_xml_declaration_whitespace() {
    for (whitespace, padded) in ["\n", "\t", "\r", " "].into_iter().flat_map(|whitespace| [false, true].map(move |padded| (whitespace, padded))) {
        let mut i = input();
        i.limits.max_metadata_bytes = 4096;
        i.primary.bytes = scene_with(r#"<include src="inc.xml"/>"#);
        let document = format!("<?xml{whitespace}version=\"1.0\"?><assets><data src=\"data.json\"/></assets>");
        // A legal declaration padded to the input cap must be stripped before wrapping.
        let padding = " ".repeat(if padded { i.limits.max_metadata_bytes - document.len() } else { 0 });
        i.files.push(FileInput { path: "inc.xml".into(), bytes: document.replacen("?>", &format!("{padding}?>"), 1).into_bytes(), role: "include".into() });
        let packed = pack(&i).expect("XML declaration whitespace must be accepted in includes");
        assert!(read(&packed, &i.limits).is_ok());
        let missing = rewrite(&packed, |entries, manifest| {
            entries.retain(|e| e.path != "project/data.json");
            manifest["files"].as_array_mut().unwrap().retain(|f| f["path"] != "data.json");
        });
        assert!(read(&missing, &i.limits).is_err(), "the included reference must still be scanned");
    }
}
#[test]
fn materialx_xml_declaration_whitespace() {
    for (whitespace, padded) in ["\n", "\t", "\r", " "].into_iter().flat_map(|whitespace| [false, true].map(move |padded| (whitespace, padded))) {
        let mut i = input();
        i.limits.max_metadata_bytes = 4096;
        i.primary.bytes = scene_with(r#"<mesh src="mesh.mtlx"/>"#);
        let document = format!("<?xml{whitespace}version=\"1.0\"?><materialx><input type=\"filename\" value=\"data.json\"/></materialx>");
        let padding = " ".repeat(if padded { i.limits.max_metadata_bytes - document.len() } else { 0 });
        i.files.push(FileInput { path: "mesh.mtlx".into(), bytes: document.replacen("?>", &format!("{padding}?>"), 1).into_bytes(), role: "asset".into() });
        let packed = pack(&i).expect("XML declaration whitespace must be accepted in MaterialX");
        assert!(read(&packed, &i.limits).is_ok());
        i.files.remove(0);
        assert!(pack(&i).is_err(), "MaterialX references must still be scanned");
    }
}
#[test]
fn package_readme_contains_schema_mit_notice() {
    let i = input();
    let packed = pack(&i).unwrap();
    let package = read(&packed, &i.limits).expect("README and schema metadata hashes must verify");
    let readme = &package.entries.iter().find(|e| e.path == "README.md").unwrap().bytes;
    let readme = std::str::from_utf8(readme).unwrap();
    assert!(readme.contains("`vpkg.schema.json` from sr-core"));
    // Exact upstream notice, including its copyright, permission and warranty text.
    assert!(readme.contains(include_str!("../../interchange/schema/sr-core/LICENSE")));
    assert_eq!(packed, pack(&i).unwrap());
}
#[test]
fn sequence_pattern_only_applies_to_basename() {
    let mut i = input();
    i.primary.bytes = scene_with(r#"<imageSequence src="frames#1/frame_%02d.png"/>"#);
    i.files = vec![FileInput { path: "frames#1/frame_01.png".into(), bytes: vec![], role: "asset".into() }];
    assert!(pack(&i).is_ok());
}
#[test]
fn inline_data_is_not_a_file_path() {
    let mut i = input();
    i.primary.bytes = scene_with(&format!("<image src=\"data:image/png;base64,{}\"/>", "A".repeat(1100)));
    assert!(pack(&i).is_ok());
}

fn unicode_input() -> PackInput {
    let mut i = input();
    i.primary.path = "é.scene".into();
    i.primary.bytes = scene_with(r#"<data src="données.json"/>"#);
    i.files[0].path = "données.json".into();
    i
}

#[test]
#[ignore = "opt-in: set SR_CORE_DIR to an sr-core checkout; runs Python"]
fn python_verifies_written_package() {
    let core = std::env::var_os("SR_CORE_DIR").expect("set SR_CORE_DIR");
    let root = std::env::temp_dir().join(format!("filmcraft-vpkg-oracle-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    for (name, input) in [("ascii", input()), ("unicode", unicode_input())] {
        let path = root.join(format!("{name}.vpkg.zip"));
        std::fs::write(&path, pack(&input).unwrap()).unwrap();
        let result = std::process::Command::new("python3")
            .args(["-m", "sr_core.vpkg", "verify"])
            .arg(&path)
            .current_dir(&core)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .unwrap();
        assert!(result.status.success(), "{name}: {}\n{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
    }
    std::fs::remove_dir_all(&root).unwrap();
}
#[test]
#[ignore = "emit a hex package for an external Python oracle without shared filesystems"]
fn emit_oracle_package() {
    println!("VPKG_HEX={}", filmcraft_bitstream::sha256::to_hex(&pack(&input()).unwrap()));
    println!("VPKG_UTF8_HEX={}", filmcraft_bitstream::sha256::to_hex(&pack(&unicode_input()).unwrap()));
}

#[test]
fn unicode_generated_globs_match_characters() {
    let b = rewrite(ORACLE, |entries, m| {
        let scene = scene_with(r#"<image src="generated/é.png"/>"#);
        entries.iter_mut().find(|e| e.path == "project/main.scene").unwrap().bytes = scene.clone();
        m["files"][0]["size"] = json!(scene.len());
        m["files"][0]["sha256"] = json!(filmcraft_bitstream::sha256::to_hex(&filmcraft_bitstream::sha256::sha256(&scene)));
        m["pipeline"] = json!({"steps":[{"id":"a","kind":"command","run":["generate"],"outputs":["generated/?.png"]}]});
    });
    assert!(read(&b, &Limits::default()).is_ok());
}
