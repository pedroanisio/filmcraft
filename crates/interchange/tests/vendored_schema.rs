//! The vendored sr-core schema (`schema/sr-core/`) is byte-for-byte what upstream published:
//! every file listed in its `SHA256SUMS` hashes to the listed value.

use filmcraft_bitstream::sha256::{sha256, to_hex};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/schema/sr-core");

#[test]
fn vendored_schema_matches_sha256sums() {
    let sums = std::fs::read_to_string(format!("{DIR}/SHA256SUMS")).expect("SHA256SUMS");
    let mut checked = Vec::new();
    for line in sums.lines().filter(|l| !l.trim().is_empty()) {
        let (want, name) = line.split_once(char::is_whitespace).expect("`<hash>  <file>` line");
        let name = name.trim_start().trim_start_matches('*');
        let bytes = std::fs::read(format!("{DIR}/{name}")).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(to_hex(&sha256(&bytes)), want, "{name} differs from upstream: re-vendor it, never edit it");
        checked.push(name.to_string());
    }
    checked.sort();
    assert_eq!(checked, ["scene-render.sch", "scene-render.xsd"]);
}
