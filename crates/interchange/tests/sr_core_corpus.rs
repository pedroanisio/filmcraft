//! Optional pinned sr-core corpus and mandatory synthetic scene regressions.
//! Run with SR_CORE_DIR pointing at babeccf0ca0cb2603b8f6a1d3cb07388b7d7a2cc.
#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use filmcraft_interchange::{Error, ExportOptions, Format, ImportOptions, Imported, Report, scene};
use serde_json::Value;

const PINNED_REV: &str = "babeccf0ca0cb2603b8f6a1d3cb07388b7d7a2cc";
// Keep in step with scene_conformance.rs: renderer/asset/build-stage findings are not validation failures.
const NOT_PORTED: &[&str] = &["E19", "E20", "E21", "PREC1", "SA01", "SA02", "SUP-REPORTED", "W04"];
const NOT_PORTED_PREFIXES: &[&str] = &["INERT-"];

#[derive(Debug)]
struct Case {
    name: String,
    xml: String,
    expected: Option<Value>,
}

#[derive(Default)]
struct Summary {
    cases: usize,
    imported: usize,
    strict_imported: usize,
    lenient_imported: usize,
    reimported: usize,
    refused: usize,
    exported: usize,
    oracle_checked: usize,
    skipped: Vec<String>,
    failures: Vec<Failure>,
}

impl Summary {
    fn finish(&self, lane: &str) {
        eprintln!(
            "{lane}: {} cases, {} imported, {} refused as expected, {} exported, {} oracle-checked, {} skips, {} failures",
            self.cases,
            self.imported,
            self.refused,
            self.exported,
            self.oracle_checked,
            self.skipped.len(),
            self.failures.len()
        );
        eprintln!("import modes: {} strict, {} lenient; {} successful re-imports", self.strict_imported, self.lenient_imported, self.reimported);
        for reason in &self.skipped {
            eprintln!("skipped: {reason}");
        }
        for failure in &self.failures {
            eprintln!("failed: {failure}");
        }
        assert!(
            self.failures.is_empty(),
            "{} failures listed above. Coarse wall-clock budget: 120 s per case; diagnostic only, not a timing assertion.",
            self.failures.len()
        );
    }
}

#[derive(Debug)]
struct Failure {
    case: String,
    mode: &'static str,
    stage: &'static str,
    codes: Vec<String>,
    message: String,
    panic: bool,
}

impl Failure {
    fn new(stage: &'static str, message: impl Into<String>) -> Self {
        Self { case: String::new(), mode: "strict", stage, codes: Vec::new(), message: message.into(), panic: false }
    }

    fn error(stage: &'static str, error: &Error) -> Self {
        let mut failure = Self::new(stage, error.to_string());
        // Overlay errors wrap diagnostics in a message instead of Error::Invalid.
        failure.codes = match error {
            Error::Invalid(diags) => diags.iter().map(|d| d.code.clone()).collect(),
            _ => ["error[", "warning[", "note["]
                .into_iter()
                .flat_map(|prefix| failure.message.split(prefix).skip(1).filter_map(|part| part.split_once(']').map(|(code, _)| code.to_owned())))
                .collect(),
        };
        failure.codes.sort();
        failure.codes.dedup();
        failure
    }

    fn panicked(stage: &'static str, message: String) -> Self {
        Self { panic: true, ..Self::new(stage, message) }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (mode={} stage={} codes={:?}): {}", self.case, self.mode, self.stage, self.codes, self.message)
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("CARGO_TARGET_TMPDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
        let path = root.join(format!("sr-corpus-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn files(root: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![root.to_owned()];
    let mut found = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() && path.extension().and_then(|e| e.to_str()).is_some_and(|e| extensions.contains(&e)) {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn json(path: &Path) -> Result<Value, String> {
    serde_json::from_str(&text(path)?).map_err(|e| format!("{}: {e}", path.display()))
}

fn discover(root: &Path) -> Result<Vec<Case>, String> {
    let expected = json(&root.join("conformance/expected.json"))?;
    let mut cases = Vec::new();
    for path in files(&root.join("conformance/cases"), &["xml"])? {
        let stem = path.file_stem().and_then(|s| s.to_str()).ok_or("non-UTF8 case name")?;
        cases.push(Case {
            name: path.strip_prefix(root).unwrap_or(&path).display().to_string(),
            xml: text(&path)?,
            expected: Some(expected["cases"][stem].clone()),
        });
    }
    for path in files(&root.join("conformance/srep_cases"), &["json", "xml", "scene"])? {
        let name = path.strip_prefix(root).unwrap_or(&path).display().to_string();
        if path.extension().is_some_and(|e| e == "json") {
            let entries = json(&path)?;
            for (id, entry) in entries.as_object().ok_or_else(|| format!("{name}: expected SREP case object"))? {
                let Some(xml) = entry.get("xml").and_then(Value::as_str) else { continue };
                cases.push(Case { name: format!("{name}::{id}"), xml: xml.into(), expected: Some(entry["expected"].clone()) });
            }
        } else {
            cases.push(Case { name, xml: text(&path)?, expected: Some(Value::Null) });
        }
    }
    if root.join("docs").is_dir() {
        for path in files(&root.join("docs"), &["xml", "scene"])? {
            let xml = text(&path)?;
            if path.extension().is_some_and(|e| e == "scene") || xml.contains("<scene") {
                cases.push(Case { name: path.strip_prefix(root).unwrap_or(&path).display().to_string(), xml, expected: Some(Value::Null) });
            }
        }
    }
    Ok(cases)
}

fn strings(value: &Value, key: &str) -> Vec<String> {
    value.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
}

fn skip_reason(entry: &Value) -> Option<String> {
    let findings = &entry["findings"];
    let codes = strings(findings, "codes")
        .into_iter()
        .chain(strings(findings, "absent"))
        .filter(|c| NOT_PORTED.contains(&c.as_str()) || NOT_PORTED_PREFIXES.iter().any(|p| c.starts_with(p)))
        .collect::<Vec<_>>();
    if !codes.is_empty() {
        Some(format!("unported stages: {}", codes.join(", ")))
    } else if let Some(reason) = entry.get("pending") {
        Some(format!("pending expectation: {reason}"))
    } else if findings.get("via").is_some() || findings.get("messages").is_some() {
        Some("render/message expectation, not a validation finding".into())
    } else {
        None
    }
}

fn supported_codes(entry: &Value) -> Vec<String> {
    let findings = &entry["findings"];
    if entry.get("pending").is_some() || findings.get("via").is_some() || findings.get("messages").is_some() {
        return Vec::new();
    }
    strings(findings, "codes").into_iter().filter(|c| !NOT_PORTED.contains(&c.as_str()) && !NOT_PORTED_PREFIXES.iter().any(|p| c.starts_with(p))).collect()
}

fn guarded<T>(stage: &str, f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Result<T, String> {
    std::panic::catch_unwind(f).map_err(|p| {
        let message = p.downcast_ref::<String>().map(String::as_str).or_else(|| p.downcast_ref::<&str>().copied()).unwrap_or("non-string panic");
        format!("{stage} panicked: {message}")
    })
}

fn clip_count(imported: &Imported, exclude_background: bool) -> Result<usize, String> {
    let id = imported.sequences.first().ok_or("no imported sequence")?;
    let seq = imported.project.sequence(*id).ok_or("missing imported sequence")?;
    Ok(seq.all_tracks().flat_map(|t| &t.items).filter(|c| !exclude_background || c.name != "Scene Background").count())
}

fn clip_losses(report: &Report) -> usize {
    // Appearance/key/transition warnings do not excuse missing clips.
    const CLIP_LOSSES: &[&str] = &[
        "invalid or zero-length clip interval skipped",
        "clip interval collapsed at nearest representable times: skipped",
        "missing source item skipped",
        "subclip reference cycle or depth limit: clip skipped",
        "synthetic media without an equivalent scene asset skipped",
        "image sequence media without an explicit filename pattern skipped",
        "media with invalid dimensions or frame rate skipped",
        "video with unknown or unrepresentable duration skipped",
        "media without video or audio skipped",
        "invalid clip speed or source in-point skipped",
        "audio-only source on a picture track skipped",
        "adjustment layer or unsupported source skipped",
        "invalid source dimensions skipped",
        "unrepresentable source span skipped",
        "unrepresentable source time skipped",
        "nested sequence cycle or depth limit: clip skipped",
        "multicam, reversed or held nested sequence skipped",
        "unrepresentable nested clock skipped",
        "duplicate clip id skipped",
        "negative audio source time skipped",
        "held audio clip omitted as silence",
        "invalid audio speed skipped",
        "unrepresentable audio clock skipped",
        "zero-length audio source span skipped",
        "nested audio sequence cycle or depth limit: clip skipped",
        "reversed or multicam nested audio skipped",
        "unrepresentable audio time skipped",
        "audio clip without a file audio source skipped",
        "audio speed below the scene minimum skipped",
    ];
    report.entries.iter().filter(|e| CLIP_LOSSES.contains(&e.message.as_str())).map(|e| e.count as usize).sum()
}

struct Oracle {
    root: PathBuf,
    scratch: Scratch,
}

impl Oracle {
    fn new(root: &Path) -> Result<Option<Self>, String> {
        let probe = command_with_deadline(Command::new("python3").args(["-c", "import lxml"]), Duration::from_secs(120));
        if probe.as_ref().is_err_and(|e| e.contains("timed out")) {
            return Err(probe.unwrap_err());
        }
        if !probe.is_ok_and(|o| o.status.success()) {
            eprintln!("schema oracle skipped: python3/lxml unavailable");
            return Ok(None);
        }
        Ok(Some(Self { root: root.to_owned(), scratch: Scratch::new() }))
    }

    fn check(&self, bytes: &[u8]) -> Result<(), String> {
        let path = self.scratch.0.join("export.scene");
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        let out = command_with_deadline(
            Command::new("python3").args(["-m", "sr_core.scene", "check", "--schema"]).arg(&path).env("PYTHONPATH", &self.root).current_dir(&self.scratch.0),
            Duration::from_secs(120),
        )
        .map_err(|e| format!("schema oracle: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("schema oracle {}: {} {}", out.status, String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
        }
    }
}

fn pipeline(case: &Case, oracle: Option<&Oracle>, summary: &mut Summary) {
    summary.cases += 1;
    let start = summary.failures.len();
    if let Some(reason) = case.expected.as_ref().and_then(skip_reason) {
        summary.skipped.push(format!("{}: {reason}; only unsupported diagnostic assertions skipped; validity retained", case.name));
    }
    let mut strict_succeeded = false;
    for (mode, lenient) in [("strict", false), ("lenient", true)] {
        let mode_start = summary.failures.len();
        match guarded("import", || scene::import(case.xml.as_bytes(), &ImportOptions { lenient, ..Default::default() })) {
            Err(e) => summary.failures.push(Failure::panicked("import", e)),
            Ok(Err(e)) => {
                let mut failure = Failure::error("import", &e);
                if lenient {
                    if strict_succeeded {
                        summary.failures.push(failure);
                    }
                } else if let Some(expected) = &case.expected {
                    if expected["findings"]["valid"].as_bool() == Some(false) {
                        let wanted = supported_codes(expected);
                        if matches!(&e, Error::Invalid(diags) if !diags.is_empty() && (wanted.is_empty() || diags.iter().any(|d| wanted.contains(&d.code)))) {
                            summary.refused += 1;
                        } else {
                            failure.message = format!("expected Error::Invalid with one of {wanted:?}; got {}", failure.message);
                            summary.failures.push(failure);
                        }
                    } else {
                        summary.failures.push(failure);
                    }
                } else {
                    summary.refused += 1;
                }
            }
            Ok(Ok((imported, _, _))) => {
                summary.imported += 1;
                if lenient {
                    summary.lenient_imported += 1;
                } else {
                    summary.strict_imported += 1;
                    strict_succeeded = true;
                }
                if !lenient && case.expected.as_ref().is_some_and(|e| e["findings"]["valid"].as_bool() == Some(false)) {
                    summary.failures.push(Failure::new("import", "invalid case imported strictly"));
                }
                roundtrip(&imported, oracle, summary);
            }
        }
        for failure in &mut summary.failures[mode_start..] {
            failure.mode = mode;
        }
    }
    for failure in &mut summary.failures[start..] {
        failure.case.clone_from(&case.name);
    }
}

fn roundtrip(imported: &Imported, oracle: Option<&Oracle>, summary: &mut Summary) {
    let Some(sequence) = imported.sequences.first() else {
        summary.failures.push(Failure::new("export", "no sequence to export"));
        return;
    };
    // scene::export::export is crate-private; this public dispatcher invokes precisely that exporter.
    let (bytes, report) = match guarded("export", || filmcraft_interchange::export(&imported.project, *sequence, Format::Scene, &ExportOptions::default())) {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            summary.failures.push(Failure::error("export", &e));
            return;
        }
        Err(e) => {
            summary.failures.push(Failure::panicked("export", e));
            return;
        }
    };
    summary.exported += 1;
    if let Some(oracle) = oracle {
        summary.oracle_checked += 1;
        if let Err(e) = oracle.check(&bytes) {
            summary.failures.push(Failure::new("oracle", e));
        }
    }
    for lenient in [false, true] {
        match guarded("re-import", || scene::import(&bytes, &ImportOptions { lenient, ..Default::default() })) {
            Ok(Ok((reimported, _, _))) => {
                summary.reimported += 1;
                // Export emits the original matte as content. XSD projectType adds a NEW default black matte;
                // exclude only the newly materialized Scene Background from the re-import count.
                match clip_count(imported, false).and_then(|before| clip_count(&reimported, true).map(|after| (before, after))) {
                    Ok((before, after)) => {
                        let losses = clip_losses(&report);
                        if after > before || before.saturating_sub(after) > losses {
                            summary.failures.push(Failure::new("re-import", format!("clip count {before} -> {after}, only {losses} reported clip losses")));
                        }
                    }
                    Err(e) => summary.failures.push(Failure::new("re-import", e)),
                }
            }
            Ok(Err(e)) => {
                let mut failure = Failure::error("re-import", &e);
                failure.message = format!("lenient={lenient}: {}", failure.message);
                summary.failures.push(failure);
            }
            Err(e) => summary.failures.push(Failure::panicked("re-import", e)),
        }
    }
}

#[test]
fn sr_core_corpus() {
    let Some(root) = std::env::var_os("SR_CORE_DIR") else {
        eprintln!("SR_CORE_DIR not set: sr-core corpus skipped");
        return;
    };
    let root = std::fs::canonicalize(root).expect("SR_CORE_DIR must name a readable checkout");
    let revision = std::env::var("SR_CORE_REV").ok().or_else(|| {
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    });
    eprintln!("sr-core revision: {} (pinned {PINNED_REV})", revision.as_deref().unwrap_or("unavailable"));
    if let Some(revision) = revision {
        assert_eq!(revision, PINNED_REV, "corpus revision must match the vendored schema");
    }
    let cases = discover(&root).unwrap();
    assert!(!cases.is_empty(), "SR_CORE_DIR contained no scene cases");
    let mut summary = Summary::default();
    let oracle = match Oracle::new(&root) {
        Ok(oracle) => oracle,
        Err(e) => {
            summary.failures.push(Failure::new("oracle", e));
            None
        }
    };
    for case in cases {
        pipeline(&case, oracle.as_ref(), &mut summary);
        if summary.cases.is_multiple_of(100) {
            eprintln!("sr-core progress: {} cases, {} oracle-checked, {} failures", summary.cases, summary.oracle_checked, summary.failures.len());
        }
    }
    summary.finish("sr-core corpus");
}

#[test]
fn scene_corpus_required_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scene");
    let paths = files(&root, &["scene"]).unwrap();
    assert!(!paths.is_empty(), "mandatory fixtures must not silently disappear");
    let mut summary = Summary::default();
    for path in paths {
        let name = path.strip_prefix(&root).unwrap().display().to_string();
        let known = KNOWN_REGRESSIONS.iter().find(|known| name == format!("corpus/{}.scene", known.file));
        let case = Case { name: name.clone(), xml: text(&path).unwrap(), expected: known.map(|_| Value::Null) };
        pipeline(&case, None, &mut summary);
        if let Some(known) = known {
            summary.failures.retain(|failure| {
                let matched = known_failure(failure, &name, known);
                if matched {
                    summary.skipped.push(format!("{failure}; known regression {}; full assertion retained in its ignored test", known.file));
                }
                !matched
            });
        }
    }
    // The hand-written image fixture is one 24-frame clip, independent of implementation output.
    let values = guarded("basic.scene expected values", || {
        let (imported, _, _) = scene::import(include_bytes!("fixtures/scene/corpus/basic.scene"), &ImportOptions::default()).map_err(|e| e.to_string())?;
        let count = clip_count(&imported, false)?;
        let sequence = imported.sequences.first().and_then(|id| imported.project.sequence(*id)).ok_or("missing sequence")?;
        Ok::<_, String>((count, sequence.duration().0))
    });
    match values {
        Ok(Ok((1, filmcraft_time::TICKS_PER_SECOND))) => (),
        other => summary.failures.push(Failure::new("import", format!("basic.scene: expected one clip lasting 254016000000 ticks, got {other:?}"))),
    }
    summary.finish("required scene fixtures");
}

#[test]
fn scene_corpus_hostile_documents() {
    let mut summary = Summary::default();
    for case in hostile_documents() {
        pipeline(&case, None, &mut summary);
    }
    summary.finish("hostile scene documents");
}

#[test]
fn scene_corpus_10000_siblings_export() {
    let case = hostile_documents().into_iter().find(|c| c.name == "10000-siblings").unwrap();
    let mut summary = Summary::default();
    require_clip_roundtrip(&case, 10_000, &mut summary);
    summary.finish("10000 siblings regression");
}

#[test]
fn scene_corpus_discovery_reads_xml_json_and_docs() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    std::fs::create_dir_all(root.join("conformance/cases")).unwrap();
    std::fs::create_dir_all(root.join("conformance/srep_cases")).unwrap();
    std::fs::create_dir_all(root.join("docs/examples")).unwrap();
    let xml = include_str!("fixtures/scene/corpus/basic.scene");
    std::fs::write(root.join("conformance/cases/a.xml"), xml).unwrap();
    std::fs::write(root.join("conformance/expected.json"), r#"{"cases":{"a":{"findings":{"valid":true}}}}"#).unwrap();
    std::fs::write(root.join("conformance/srep_cases/a.json"), serde_json::json!({"embedded":{"xml":xml,"expected":{"findings":{"valid":true}}}}).to_string())
        .unwrap();
    std::fs::write(root.join("docs/examples/b.scene"), xml).unwrap();
    std::fs::write(root.join("docs/examples/irrelevant.xml"), "<not-a-scene/>").unwrap();
    let cases = discover(root).unwrap();
    assert_eq!(cases.len(), 3, "discover file-backed cases, embedded SREP scenes, and docs scenes");
    assert!(cases.iter().all(|case| case.xml.contains("<scene")));
}
fn hostile_documents() -> Vec<Case> {
    let base = include_str!("fixtures/scene/corpus/basic.scene");
    let mut cases = Vec::new();
    for (name, from, to) in [
        ("loop-u32-max", "start=\"0\"", "loop=\"4294967295\" clipOut=\"0.01\" start=\"0\""),
        ("tiny-speed", "start=\"0\"", "speed=\"1e-300\" clipOut=\"1\" start=\"0\""),
        ("zero-remap-keys", "/></composition>", "><timeRemap/></layer></composition>"),
        ("zero-fps-denominator", "fps=\"24\"", "fps=\"30000/0\""),
        // (2^53 + 1) / 254016000000, rounded only to the decimal spelling below.
        ("beyond-f64-integer-ticks", "duration=\"1\"", "duration=\"35459.1807395636219765684051398\""),
    ] {
        cases.push(Case { expected: None, name: name.into(), xml: base.replace(from, to) });
    }
    for value in ["NaN", "nan", "INF", "-INF", "inf", "Infinity", "1e309"] {
        cases.push(Case { expected: None, name: format!("nonfinite-{value}"), xml: base.replace("start=\"0\"", &format!("start=\"{value}\"")) });
    }
    cases.push(Case {
        expected: None,
        name: "matte-cycle".into(),
        xml: base.replace("<layer id=\"clip\" asset=\"picture\" start=\"0\" end=\"1\"/>", "<group id=\"a\" matte=\"b\"/><group id=\"b\" matte=\"a\"/>"),
    });
    // scene (1), composition (2), groups (3..depth): exact reader boundary, then one over.
    let limit = scene::Limits::default().max_depth;
    for depth in [limit, limit + 1] {
        let opening = (0..depth - 2).map(|i| format!("<group id=\"g{i}\">")).collect::<String>();
        let groups = format!("{opening}{}", "</group>".repeat(depth - 2));
        cases.push(Case {
            expected: None,
            name: format!("depth-{depth}"),
            xml: base.replace("<layer id=\"clip\" asset=\"picture\" start=\"0\" end=\"1\"/>", &groups),
        });
    }
    let layers = (0..10_000).map(|i| format!("<layer id=\"l{i}\" asset=\"picture\"/>")).collect::<String>();
    cases.push(Case {
        expected: None,
        name: "10000-siblings".into(),
        xml: base.replace("<layer id=\"clip\" asset=\"picture\" start=\"0\" end=\"1\"/>", &layers),
    });
    cases
}

fn regression(name: &str, xml: &str) {
    let mut summary = Summary::default();
    pipeline(&Case { name: name.into(), xml: xml.into(), expected: Some(Value::Null) }, None, &mut summary);
    summary.finish(name);
}

// Only these precise observed failures are deferred. Panics and new failure modes remain mandatory failures.
struct KnownRegression {
    file: &'static str,
    stage: &'static str,
    codes: &'static [&'static str],
    message: Option<&'static str>,
}

const KNOWN_REGRESSIONS: &[KnownRegression] = &[
    KnownRegression { file: "material-override", stage: "import", codes: &["MOV2"], message: None },
    KnownRegression {
        file: "external-expression",
        stage: "import",
        codes: &[],
        message: Some("sr-core scene parse error: overlay dependency source is outside its subtree; cannot isolate external composition dependency"),
    },
    KnownRegression { file: "geodesic-camera", stage: "import", codes: &["BH5"], message: None },
    KnownRegression { file: "voxel-palette", stage: "import", codes: &["VOX6"], message: None },
    KnownRegression { file: "generated-id", stage: "import", codes: &[], message: Some("sr-core scene parse error: overlay dependency g1 is missing") },
];

macro_rules! regression_test {
    ($name:ident, $file:literal, $cause:literal) => {
        #[test]
        #[ignore = $cause]
        fn $name() {
            regression($file, include_str!(concat!("fixtures/scene/corpus/", $file, ".scene")));
        }
    };
}
#[test]
fn scene_corpus_fractional_depth() {
    regression("fractional-depth", include_str!("fixtures/scene/corpus/fractional-depth.scene"));
}
regression_test!(scene_corpus_material_override, "material-override", "S11: overlay closure loses materialOverride material ids");
regression_test!(scene_corpus_geodesic_camera, "geodesic-camera", "S11: overlay closure loses the implicit blackHole dependency");
regression_test!(scene_corpus_voxel_palette, "voxel-palette", "S11: overlay closure loses palette material ids");
regression_test!(scene_corpus_generated_id, "generated-id", "S11: overlay closure treats program-generated ids as missing DOM ids");
#[test]
fn scene_corpus_linked_audio() {
    regression("linked-audio", include_str!("fixtures/scene/corpus/linked-audio.scene"));
}
regression_test!(scene_corpus_external_expression, "external-expression", "S11: overlay closure rejects valid sibling dependencies");

#[test]
fn scene_corpus_external_matte() {
    regression("external-matte", include_str!("fixtures/scene/corpus/external-matte.scene"));
}

// A synthetic entry for the matcher's own tests, so they don't depend on which product bugs are still open.
const SAMPLE_REGRESSION: KnownRegression = KnownRegression { file: "sample", stage: "import", codes: &["MOV2"], message: None };

fn known_failure(failure: &Failure, name: &str, known: &KnownRegression) -> bool {
    !failure.panic
        && failure.case == name
        && failure.stage == known.stage
        && failure.codes == known.codes
        && known.message.is_none_or(|message| failure.message == message)
}

#[test]
fn scene_corpus_known_failures_never_hide_panics() {
    let known = &SAMPLE_REGRESSION;
    let mut failure = Failure::new("import", "MOV2");
    failure.case = "sample".into();
    failure.codes = vec!["MOV2".into()];
    assert!(known_failure(&failure, "sample", known));
    assert!(!known_failure(&failure, "another", known));
    failure.panic = true;
    assert!(!known_failure(&failure, "sample", known), "a known diagnostic must never excuse a panic");
}

#[test]
fn scene_corpus_expectations_aggregate_and_match_codes() {
    let xml = include_str!("fixtures/scene/corpus/basic.scene").replace("fps=\"24\"", "fps=\"30000/0\"");
    let mut summary = Summary::default();
    // XSD fpsType excludes a zero denominator; the structural validator's type-facet code is S06.
    for (name, expected) in [
        ("matching", serde_json::json!({"findings":{"valid":false,"codes":["S06"]}})),
        ("wrong-code", serde_json::json!({"findings":{"valid":false,"codes":["DOES-NOT-EXIST"]}})),
        ("wrong-validity", serde_json::json!({"findings":{"valid":true}})),
    ] {
        pipeline(&Case { name: name.into(), xml: xml.clone(), expected: Some(expected) }, None, &mut summary);
    }
    assert_eq!(summary.refused, 1);
    assert_eq!(summary.failures.len(), 2, "all mismatches must be retained: {:?}", summary.failures);
    assert!(summary.failures.iter().any(|f| f.case == "wrong-code"));
    assert!(summary.failures.iter().any(|f| f.case == "wrong-validity"));
}

#[test]
fn scene_corpus_only_clip_losses_allow_missing_clips() {
    let mut report = Report::default();
    report.warn("effect without a scene equivalent dropped");
    report.warn("non-increasing animation key dropped");
    assert_eq!(clip_losses(&report), 0);
    report.warn("missing source item skipped");
    report.warn("missing source item skipped");
    assert_eq!(clip_losses(&report), 2);
}

#[test]
fn harness_unported_expectation_keeps_validity() {
    let xml = include_str!("fixtures/scene/corpus/basic.scene").replace("fps=\"24\"", "fps=\"30000/0\"");
    for findings in [
        serde_json::json!({"valid":true,"absent":["INERT-DEFAULT"]}),
        serde_json::json!({"valid":true,"via":"renderer"}),
        serde_json::json!({"valid":false,"codes":["E19","WRONG"]}),
    ] {
        let mut summary = Summary::default();
        pipeline(&Case { name: "unsupported".into(), xml: xml.clone(), expected: Some(serde_json::json!({"findings":findings})) }, None, &mut summary);
        assert!(!summary.failures.is_empty(), "unsupported codes must not drop validity or supported codes");
    }
}

#[test]
fn harness_roundtrips_both_import_modes() {
    let mut summary = Summary::default();
    pipeline(&Case { name: "basic".into(), xml: include_str!("fixtures/scene/corpus/basic.scene").into(), expected: Some(Value::Null) }, None, &mut summary);
    assert_eq!(summary.exported, 2, "strict and lenient projects must both export");
    assert!(summary.failures.is_empty());
}

#[test]
fn harness_known_failure_rejects_extra_codes_and_other_stages() {
    let known = &SAMPLE_REGRESSION;
    for (stage, codes) in [("import", vec!["MOV2".into(), "NEW".into()]), ("re-import", vec!["MOV2".into()]), ("export", vec!["MOV2".into()])] {
        let mut failure = Failure::new(stage, "diagnostic");
        failure.case = "sample".into();
        failure.codes = codes;
        assert!(!known_failure(&failure, "sample", known), "must not hide {failure}");
    }
}

fn wait_with_deadline(child: &mut Child, limit: Duration) -> Result<std::process::ExitStatus, String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(10).min(limit.saturating_sub(started.elapsed()))),
            result => {
                let reason = match result {
                    Err(e) => e.to_string(),
                    _ => format!("timed out after {limit:?}"),
                };
                let killed = child.kill();
                let reaped = child.wait();
                return Err(format!("{reason}; kill={killed:?}; reap={reaped:?}"));
            }
        }
    }
}

fn command_with_deadline(command: &mut Command, limit: Duration) -> Result<Output, String> {
    // File-backed output avoids a full stdout/stderr pipe blocking the child before it can exit.
    let scratch = Scratch::new();
    let stdout = scratch.0.join("stdout");
    let stderr = scratch.0.join("stderr");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&stdout).map_err(|e| e.to_string())?)
        .stderr(std::fs::File::create(&stderr).map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|e| e.to_string())?;
    let status = wait_with_deadline(&mut child, limit)?;
    Ok(Output { status, stdout: std::fs::read(stdout).map_err(|e| e.to_string())?, stderr: std::fs::read(stderr).map_err(|e| e.to_string())? })
}

fn require_clip_roundtrip(case: &Case, count: usize, summary: &mut Summary) {
    for lenient in [false, true] {
        let imported = guarded("import", || scene::import(case.xml.as_bytes(), &ImportOptions { lenient, ..Default::default() }))
            .expect("large sibling import must not panic")
            .expect("large sibling import must succeed")
            .0;
        assert_eq!(clip_count(&imported, false).unwrap(), count, "must import every sibling (lenient={lenient})");
    }
    pipeline(case, None, summary);
    assert_eq!(summary.exported, 2, "both sibling import modes must export: {:?}", summary.failures);
    assert_eq!(summary.reimported, 4, "both exports must re-import strictly and leniently: {:?}", summary.failures);
}

#[test]
fn harness_command_deadline() {
    let result = command_with_deadline(Command::new("sh").args(["-c", "exec sleep 120"]), std::time::Duration::ZERO);
    assert!(result.is_err_and(|e| e.contains("timed out")), "deadline must kill and reap the child");
}

#[test]
fn harness_lenient_only_success_is_roundtripped() {
    let xml = include_str!("fixtures/scene/corpus/basic.scene").replace("<project ", "<project unknown=\"true\" ");
    let mut summary = Summary::default();
    pipeline(&Case { name: "lenient-only".into(), xml, expected: Some(serde_json::json!({"findings":{"valid":false,"codes":["S04"]}})) }, None, &mut summary);
    assert_eq!(summary.strict_imported, 0);
    assert_eq!(summary.lenient_imported, 1);
    assert_eq!(summary.exported, 1);
    assert_eq!(summary.reimported, 2);
    assert!(summary.failures.is_empty(), "{:?}", summary.failures);
}

#[test]
fn harness_failure_codes_are_complete_normalized_sets() {
    let error = Error::Other("overlay: error[NEW] first; error[MOV2] second; error[NEW] duplicate".into());
    assert_eq!(Failure::error("import", &error).codes, ["MOV2", "NEW"]);
    let mut diagnostic = scene::diag::Diagnostic {
        severity: scene::diag::Severity::Error,
        code: "MOV2".into(),
        message: "first".into(),
        line: 1,
        column: 1,
        offset: 0,
        path: "/scene".into(),
        help: None,
    };
    let mut diags = vec![diagnostic.clone()];
    diagnostic.code = "NEW".into();
    diags.push(diagnostic);
    // Error's Display shows only the first diagnostic, so matching must inspect the typed error.
    assert_eq!(Failure::error("import", &Error::Invalid(diags)).codes, ["MOV2", "NEW"]);
}

#[test]
fn harness_deadline_reaps_and_captures_output() {
    let mut child = Command::new("sh").args(["-c", "exec sleep 120"]).spawn().unwrap();
    assert!(wait_with_deadline(&mut child, Duration::ZERO).unwrap_err().contains("timed out"));
    assert!(child.try_wait().unwrap().is_some(), "timed-out child must already be reaped");
    let out = command_with_deadline(Command::new("sh").args(["-c", "printf stdout; printf stderr >&2; exit 3"]), Duration::from_secs(120)).unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(out.stdout, b"stdout");
    assert_eq!(out.stderr, b"stderr");
}

#[test]
fn harness_large_case_rejects_refusal_and_wrong_count() {
    let basic = include_str!("fixtures/scene/corpus/basic.scene");
    for xml in [basic.replace("fps=\"24\"", "fps=\"30000/0\""), basic.into()] {
        let case = Case { name: "siblings-contract".into(), xml, expected: None };
        assert!(
            std::panic::catch_unwind(|| require_clip_roundtrip(&case, 10_000, &mut Summary::default())).is_err(),
            "a refused import or fewer than 10000 clips must fail the sibling regression"
        );
    }
}
