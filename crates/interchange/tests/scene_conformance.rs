//! Validation parity with the sr-core conformance kit.
//!
//! Set `SR_CORE_DIR` to an sr-core checkout to run every case of `conformance/cases/*.xml` (checked against its
//! `expected.json` entry: the findings it lists, or valid for a picture case) and every SREP case whose expectation is
//! a validation finding (`conformance/srep_cases/*.json`). Without it the test prints that it was skipped. The kit is
//! never required in CI.

use std::path::{Path, PathBuf};

use filmcraft_interchange::scene::read::{Limits, read};
use filmcraft_interchange::scene::validate::validate;
use serde_json::Value;

/// Codes produced by scene-render stages FilmCraft does not port (render-time findings such as E20 mask misses and
/// PREC1 precision loss, asset verification, build-mode programs). Cases expecting them are reported, not failed.
const NOT_PORTED: &[&str] = &["E19", "E20", "E21", "PREC1", "SA01", "SA02", "SUP-REPORTED", "W04"];

/// Code families of unported stages: `INERT-*` comes from scene-render's evaluator noticing attributes with no effect.
const NOT_PORTED_PREFIXES: &[&str] = &["INERT-"];

fn not_ported(code: &str) -> bool {
    NOT_PORTED.contains(&code) || NOT_PORTED_PREFIXES.iter().any(|p| code.starts_with(p))
}

/// Error and finding codes of one document: `None` when it does not parse as a scene document at all.
fn findings(xml: &str) -> Option<(bool, Vec<String>)> {
    let doc = read(xml, &Limits::default()).ok()?;
    let diags = validate(&doc.doc);
    Some((diags.iter().any(|d| d.is_error()), diags.into_iter().map(|d| d.code).collect()))
}

fn check(name: &str, xml: &str, expected: &Value, failures: &mut Vec<String>, unported: &mut usize) {
    let Some((invalid, codes)) = findings(xml) else {
        if expected.get("valid").and_then(Value::as_bool) != Some(false) {
            failures.push(format!("{name}: does not read as a scene document"));
        }
        return;
    };
    let strs = |k: &str| -> Vec<String> {
        expected.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    };
    let wanted = strs("codes");
    if wanted.iter().chain(&strs("absent")).any(|c| not_ported(c)) {
        *unported += 1;
        return;
    }
    let mut problems = Vec::new();
    if let Some(valid) = expected.get("valid").and_then(Value::as_bool)
        && valid == invalid
    {
        problems.push(format!("expected valid={valid}"));
    }
    for c in &wanted {
        if !codes.contains(c) {
            problems.push(format!("missing {c}"));
        }
    }
    for c in strs("absent") {
        if codes.contains(&c) {
            problems.push(format!("unexpected {c}"));
        }
    }
    if let Some(counts) = expected.get("counts").and_then(Value::as_object) {
        for (c, n) in counts {
            let got = codes.iter().filter(|x| *x == c).count();
            if Some(got as u64) != n.as_u64() {
                problems.push(format!("{c} {got} times, expected {n}"));
            }
        }
    }
    if !problems.is_empty() {
        failures.push(format!("{name}: {} (got {codes:?})", problems.join(", ")));
    }
}

/// Whether an expectation is one validation can meet: not pending, not a render-time finding, not a message match.
fn checks_validation(entry: &Value, findings: &Value) -> bool {
    entry.get("pending").is_none() && findings.get("via").is_none() && findings.get("messages").is_none()
}

fn kit() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("SR_CORE_DIR")?).join("conformance");
    dir.is_dir().then_some(dir)
}

fn sorted_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(dir).unwrap().filter_map(|e| Some(e.ok()?.path())).filter(|p| p.extension().is_some_and(|e| e == ext)).collect();
    files.sort();
    files
}

#[test]
fn sr_core_conformance_findings() {
    let Some(dir) = kit() else {
        eprintln!("SR_CORE_DIR not set: sr-core conformance parity skipped");
        return;
    };
    let (mut failures, mut unported, mut checked) = (Vec::new(), 0usize, 0usize);
    let spec: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("expected.json")).unwrap()).unwrap();
    let valid = serde_json::json!({ "valid": true });
    let mut seen = std::collections::HashSet::new();
    for path in sorted_files(&dir.join("cases"), "xml") {
        let xml = std::fs::read_to_string(&path).unwrap();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let entry = &spec["cases"][&name];
        let expected = match entry.get("findings") {
            Some(findings) if !checks_validation(entry, findings) => continue,
            Some(findings) => findings,
            // a picture case renders, so it validates
            None => &valid,
        };
        check(&name, &xml, expected, &mut failures, &mut unported);
        seen.insert(name);
        checked += 1;
    }
    for path in sorted_files(&dir.join("srep_cases"), "json") {
        let cases: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for (name, case) in cases.as_object().unwrap() {
            let expected = &case["expected"];
            let Some(findings) = expected.get("findings") else { continue };
            if seen.contains(name) || !checks_validation(expected, findings) {
                continue;
            }
            check(name, case["xml"].as_str().unwrap(), findings, &mut failures, &mut unported);
            checked += 1;
        }
    }
    eprintln!("sr-core conformance: {checked} cases, {unported} need unported stages, {} failures", failures.len());
    assert!(failures.is_empty(), "{} of {checked} cases disagree:\n{}", failures.len(), failures.join("\n"));
}
