#![cfg(test)]
use filmcraft_vpkg::{Limits, manifest, zip};
use serde_json::{Value, json};

fn u16le(b: &mut Vec<u8>, n: u16) {
    b.extend(n.to_le_bytes());
}
fn u32le(b: &mut Vec<u8>, n: u32) {
    b.extend(n.to_le_bytes());
}
// Hand-written ZIP records: no dependency on the writer under test.
fn archive(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut b = Vec::new();
    let mut cd = Vec::new();
    for (name, data, compressed) in entries {
        let offset = b.len() as u32;
        let payload = if *compressed { filmcraft_bitstream::flate::deflate(data) } else { data.to_vec() };
        let crc = filmcraft_bitstream::crc32(data);
        u32le(&mut b, 0x04034b50);
        u16le(&mut b, 20);
        u16le(&mut b, 0);
        u16le(&mut b, if *compressed { 8 } else { 0 });
        u16le(&mut b, 0);
        u16le(&mut b, 33);
        u32le(&mut b, crc);
        u32le(&mut b, payload.len() as u32);
        u32le(&mut b, data.len() as u32);
        u16le(&mut b, name.len() as u16);
        u16le(&mut b, 0);
        b.extend(name.as_bytes());
        b.extend(&payload);
        u32le(&mut cd, 0x02014b50);
        u16le(&mut cd, 0x0314);
        u16le(&mut cd, 20);
        u16le(&mut cd, 0);
        u16le(&mut cd, if *compressed { 8 } else { 0 });
        u16le(&mut cd, 0);
        u16le(&mut cd, 33);
        u32le(&mut cd, crc);
        u32le(&mut cd, payload.len() as u32);
        u32le(&mut cd, data.len() as u32);
        u16le(&mut cd, name.len() as u16);
        for _ in 0..4 {
            u16le(&mut cd, 0);
        }
        u32le(&mut cd, 0o100644 << 16);
        u32le(&mut cd, offset);
        cd.extend(name.as_bytes());
    }
    let start = b.len() as u32;
    let size = cd.len() as u32;
    b.extend(cd);
    u32le(&mut b, 0x06054b50);
    u16le(&mut b, 0);
    u16le(&mut b, 0);
    u16le(&mut b, entries.len() as u16);
    u16le(&mut b, entries.len() as u16);
    u32le(&mut b, size);
    u32le(&mut b, start);
    u16le(&mut b, 0);
    b
}
fn cd(b: &[u8]) -> usize {
    b.windows(4).position(|s| s == b"PK\x01\x02").unwrap()
}
fn reject(b: &[u8]) {
    assert!(zip::read(b, &Limits::default()).is_err());
}
fn utf8_flags(bytes: &mut [u8]) {
    let headers: Vec<_> = bytes
        .windows(4)
        .enumerate()
        .filter_map(|(i, s)| match s {
            b"PK\x03\x04" => Some(i + 6),
            b"PK\x01\x02" => Some(i + 8),
            _ => None,
        })
        .collect();
    for at in headers {
        bytes[at..at + 2].copy_from_slice(&0x800u16.to_le_bytes());
    }
}
#[test]
fn zip_non_ascii_requires_utf8_flag() {
    // UTF-8 é is C3 A9; without bit 11 those bytes do not declare that name.
    let mut bytes = archive(&[("project/é.scene", b"scene", false)]);
    reject(&bytes);
    utf8_flags(&mut bytes);
    assert_eq!(zip::read(&bytes, &Limits::default()).unwrap()[0].path, "project/é.scene");
    let central = cd(&bytes);
    for at in [6, central + 8] {
        let mut mismatch = bytes.clone();
        mismatch[at..at + 2].fill(0);
        reject(&mismatch);
    }
}
#[test]
fn zip_writer_marks_non_ascii_utf8() {
    let bytes = zip::write(&[zip::Input { path: "project/é.scene", bytes: b"scene", executable: false }], &Limits::default(), None).unwrap();
    let central = cd(&bytes);
    for at in [6, central + 8] {
        assert_eq!(u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap()) & 0x800, 0x800);
    }
    assert_eq!(zip::read(&bytes, &Limits::default()).unwrap()[0].path, "project/é.scene");
}
#[test]
fn zip_sharp_s_case_collision() {
    let mut bytes = archive(&[("project/ß.png", b"", false), ("project/ẞ.png", b"", false)]);
    utf8_flags(&mut bytes);
    reject(&bytes);
    assert!(
        zip::write(
            &[zip::Input { path: "project/ß.png", bytes: b"", executable: false }, zip::Input { path: "project/ẞ.png", bytes: b"", executable: false },],
            &Limits::default(),
            None
        )
        .is_err()
    );
}
#[test]
fn zip_sharp_s_parent_collision() {
    let mut bytes = archive(&[("project/ß", b"", false), ("project/ẞ/image.png", b"", false)]);
    utf8_flags(&mut bytes);
    reject(&bytes);
}
#[test]
fn zip_stored_and_deflated() {
    let b = archive(&[("a.txt", b"abc", false), ("b.txt", b"hello hello", true)]);
    let entries = zip::read(&b, &Limits::default()).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].bytes, b"abc");
    assert_eq!(entries[1].bytes, b"hello hello");
}
#[test]
fn zip_reproducible_writer() {
    let files = [zip::Input { path: "z.png", bytes: b"abc", executable: false }, zip::Input { path: "a.sh", bytes: b"echo hi", executable: true }];
    let b = zip::write(&files, &Limits::default(), None).unwrap();
    assert_eq!(b, zip::write(&files, &Limits::default(), None).unwrap());
    // DOS 1980-01-01 = date 33, time zero; first payload is stored.
    assert_eq!(&b[8..14], &[0, 0, 0, 0, 33, 0]);
    let e = zip::read(&b, &Limits::default()).unwrap();
    assert_eq!(e[0].path, "z.png");
    assert!(e[1].executable);
    let dated = zip::write(&files, &Limits::default(), Some(946684800)).unwrap();
    // 2000-01-01: (20 << 9) | 1 << 5 | 1 = 10273.
    assert_eq!(&dated[12..14], &10273u16.to_le_bytes());
    assert!(zip::write(&files, &Limits::default(), Some(u64::MAX)).is_err());
}
#[test]
fn zip_truncated_no_eocd_and_comment() {
    let b = archive(&[("x", b"x", false)]);
    for n in 0..b.len() {
        reject(&b[..n]);
    }
    let mut b = b;
    let n = b.len();
    b[n - 2] = 1;
    reject(&b);
    b.push(b'x');
    assert!(zip::read(&b, &Limits::default()).is_ok());
    b.push(0);
    reject(&b);
}
#[test]
fn zip_bad_crc_and_header_mismatch() {
    let base = archive(&[("x", b"abc", false)]);
    for offset in [30, 31, 6, 8, 14, 18, 22] {
        let mut b = base.clone();
        b[offset] ^= 1;
        reject(&b);
    }
}
#[test]
fn zip_duplicate_and_unsafe_names() {
    for names in [("x", "x"), ("X", "x"), ("a/b", "a"), ("A", "a/b")] {
        reject(&archive(&[(names.0, b"", false), (names.1, b"", false)]));
    }
    for name in ["../x", "/x", "C:x", "a\\b", "a//b", "a/", "./a", "a\0b", "a\nb", "a\u{7f}b"] {
        reject(&archive(&[(name, b"", false)]));
        assert!(!zip::safe_path(name), "{name}");
    }
}
#[test]
fn zip_symlink_zip64_encrypted_multidisk() {
    let base = archive(&[("x", b"x", false)]);
    let c = cd(&base);
    let end = base.len() - 22;
    let mut b = base.clone();
    b[c + 38..c + 42].copy_from_slice(&(0o120777u32 << 16).to_le_bytes());
    reject(&b);
    let mut b = base.clone();
    b[c + 24..c + 28].fill(255);
    reject(&b);
    let mut b = base.clone();
    b[end + 10..end + 12].fill(255);
    reject(&b);
    let mut b = base.clone();
    b[6] = 1;
    b[c + 8] = 1;
    reject(&b);
    let mut b = base;
    b[end + 4] = 1;
    reject(&b);
}
#[test]
fn zip_overlaps_and_data_past_directory() {
    let base = archive(&[("x", b"x", false), ("y", b"y", false)]);
    let c = cd(&base);
    let mut b = base.clone();
    b[c + 47 + 42..c + 47 + 46].fill(0);
    reject(&b);
    let mut b = base.clone();
    b[c + 20..c + 24].copy_from_slice(&1000u32.to_le_bytes());
    reject(&b);
    let mut b = base;
    let end = b.len() - 22;
    b.insert(end, 0);
    reject(&b);
}
#[test]
fn zip_actual_inflate_budgets_and_trailing_stream() {
    let mut b = archive(&[("x", &vec![0; 4096], true)]);
    let c = cd(&b);
    b[22..26].copy_from_slice(&1u32.to_le_bytes());
    b[c + 24..c + 28].copy_from_slice(&1u32.to_le_bytes());
    let limits = Limits { max_entry_bytes: 16, ..Limits::default() };
    assert!(zip::read(&b, &limits).is_err());
    let b = archive(&[("x", b"abc", false), ("y", b"abc", true)]);
    for limits in [
        Limits { max_total_bytes: 5, ..Limits::default() },
        Limits { max_entries: 1, ..Limits::default() },
        Limits { max_memory_bytes: b.len() + 3, ..Limits::default() },
        Limits { max_name_bytes: 0, ..Limits::default() },
    ] {
        assert!(zip::read(&b, &limits).is_err());
    }
    let mut b = archive(&[("x", b"abc", true)]);
    let c = cd(&b);
    let end = b.len() - 22;
    let compressed = u32::from_le_bytes(b[18..22].try_into().unwrap());
    b[18..22].copy_from_slice(&(compressed + 1).to_le_bytes());
    b[c + 20..c + 24].copy_from_slice(&(compressed + 1).to_le_bytes());
    b[end + 16..end + 20].copy_from_slice(&((c + 1) as u32).to_le_bytes());
    b.insert(c, 0);
    reject(&b);
}
#[test]
fn zip_data_descriptors() {
    for signed in [false, true] {
        let mut b = archive(&[("x", b"abc", true)]);
        let c = cd(&b);
        let end = b.len() - 22;
        let mut descriptor = Vec::new();
        if signed {
            u32le(&mut descriptor, 0x08074b50);
        }
        descriptor.extend(&b[14..26]);
        b[6] = 8;
        b[14..26].fill(0);
        b[c + 8] = 8;
        b[end + 16..end + 20].copy_from_slice(&((c + descriptor.len()) as u32).to_le_bytes());
        b.splice(c..c, descriptor);
        assert_eq!(zip::read(&b, &Limits::default()).unwrap()[0].bytes, b"abc");
        b[c + usize::from(signed) * 4] ^= 1;
        reject(&b);
    }
}
fn manifest_value() -> Value {
    json!({"format":"scene-video-package","formatVersion":"1.1","package":{"id":"demo","version":"1.0.0","title":"Demo"},"scenes":[{"path":"main.scene","role":"primary"}],"files":[]})
}
fn parse(v: &Value) -> Result<manifest::Manifest, filmcraft_vpkg::Error> {
    manifest::read(&serde_json::to_vec(v).unwrap(), &Limits::default())
}
#[test]
fn manifest_versions_unknown_fields_and_schema() {
    let mut m = manifest_value();
    assert!(parse(&m).is_ok());
    m["formatVersion"] = json!("2.0");
    assert!(parse(&m).is_err());
    m["formatVersion"] = json!("1.2");
    m["future"] = json!({"keep":3});
    let read = parse(&m).unwrap();
    assert_eq!(read.raw, m);
    assert!(!read.warnings.is_empty());
    m["formatVersion"] = json!("1.1");
    assert!(parse(&m).is_err());
    for (pointer, value) in [
        ("/format", json!("wrong")),
        ("/package/id", json!("Upper")),
        ("/package/version", json!("1.0")),
        ("/package/title", json!("")),
        ("/scenes/0/path", json!("../x")),
        ("/scenes/0/role", json!("weird")),
    ] {
        let mut m = manifest_value();
        *m.pointer_mut(pointer).unwrap() = value;
        assert!(parse(&m).is_err(), "{pointer}");
    }
}
#[test]
fn manifest_primary_files_and_hashes() {
    let mut m = manifest_value();
    m["scenes"].as_array_mut().unwrap().push(json!({"path":"b.scene","role":"primary"}));
    assert!(parse(&m).is_err());
    for file in [
        json!({"path":"x","size":-1,"sha256":"a".repeat(64),"role":"asset"}),
        json!({"path":"x","size":0,"sha256":"g".repeat(64),"role":"asset"}),
        json!({"path":"../x","size":0,"sha256":"a".repeat(64),"role":"asset"}),
    ] {
        let mut m = manifest_value();
        m["files"] = json!([file]);
        assert!(parse(&m).is_err());
    }
    let mut m = manifest_value();
    let f = json!({"path":"x","size":0,"sha256":"a".repeat(64),"role":"asset"});
    m["files"] = json!([f, f]);
    assert!(parse(&m).is_err());
}
#[test]
fn manifest_pipeline_semantics() {
    for steps in [
        json!([{"id":"a","kind":"command","run":["x"],"needs":["b"]},{"id":"b","kind":"command","run":["x"],"needs":["a"]}]),
        json!([{"id":"a","kind":"command","run":["x"],"needs":["missing"]}]),
        json!([{"id":"a","kind":"command","run":["x"]},{"id":"a","kind":"command","run":["x"]}]),
        json!([{"id":"a","kind":"command"}]),
        json!([{"id":"a","kind":"render","render":{"scene":"other.scene"}}]),
    ] {
        let mut m = manifest_value();
        m["pipeline"] = json!({"steps":steps});
        assert!(parse(&m).is_err());
    }
    let mut m = manifest_value();
    m["pipeline"] = json!({"steps":[{"id":"a","kind":"command","run":["x"]},{"id":"b","kind":"render","render":{"scene":"main.scene"},"needs":["a"]}]});
    assert!(parse(&m).is_ok());
}
