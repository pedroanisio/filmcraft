//! Typed manifest fields alongside the complete, preserved JSON document.
// Adapted policies from sr-core sr_core/vpkg; see README.md and ATTRIBUTION.md.
// MIT License
//
// Copyright (c) 2026 Pedro Anisio Silva
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use crate::{Diagnostic, Error, Limits, invalid};
use crate::{Severity, finding, zip};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

/// A validated manifest, retaining fields unknown to this reader.
#[derive(Debug)]
pub struct Manifest {
    pub raw: Value,
    pub data: Data,
    pub minor: u32,
    pub warnings: Vec<Diagnostic>,
}
/// The fields used by package verification.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Data {
    pub format: String,
    pub format_version: String,
    pub package: Metadata,
    pub scenes: Vec<Scene>,
    pub files: Vec<File>,
    #[serde(default)]
    pub fetch: Vec<Fetch>,
    #[serde(default)]
    pub pipeline: Pipeline,
    #[serde(default)]
    pub packed: Packed,
}
/// Package identity supplied by the author.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Metadata {
    pub id: String,
    pub version: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
}
/// A scene and its intended use.
#[derive(Debug, Deserialize, Serialize)]
pub struct Scene {
    pub path: String,
    pub role: String,
}
/// One hashed project file.
#[derive(Debug, Deserialize, Serialize)]
pub struct File {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub role: String,
}
/// A pinned input supplied separately.
#[derive(Debug, Deserialize, Serialize)]
pub struct Fetch {
    pub path: String,
    pub url: String,
    pub sha256: String,
}
/// Build steps, inspected but never executed.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Pipeline {
    #[serde(default)]
    pub steps: Vec<Step>,
}
/// A pipeline step.
#[derive(Debug, Deserialize, Serialize)]
pub struct Step {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    pub run: Option<Vec<String>>,
    pub render: Option<Render>,
}
/// A render step's scene and output.
#[derive(Debug, Deserialize, Serialize)]
pub struct Render {
    pub scene: String,
    pub to: Option<String>,
}
/// Hashes of non-project metadata.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Packed {
    pub meta_sha256: Option<std::collections::BTreeMap<String, String>>,
}

/// The unchanged sr-core package schema, also shipped inside written archives.
pub const SCHEMA: &[u8] = include_bytes!("../schema/vpkg-1.schema.json");

/// Read a packed manifest, retaining unknown fields and warning on newer 1.x additions.
pub fn read(bytes: &[u8], limits: &Limits) -> Result<Manifest, Error> {
    if bytes.len() > limits.max_metadata_bytes || bytes.len().saturating_mul(96).saturating_add(SCHEMA.len().saturating_mul(64)) > limits.max_memory_bytes {
        return Err(invalid("LIMIT", "vpkg.json", "manifest allocation budget exceeded"));
    }
    let raw: Value = serde_json::from_slice(bytes).map_err(|e| invalid("MANIFEST", "vpkg.json", &e.to_string()))?;
    let version = raw.get("formatVersion").and_then(Value::as_str).ok_or_else(|| invalid("VERSION", "$.formatVersion", "expected MAJOR.MINOR"))?;
    let (major, minor) = version.split_once('.').ok_or_else(|| invalid("VERSION", "$.formatVersion", "expected MAJOR.MINOR"))?;
    if major != "1" {
        return Err(invalid("VERSION", "$.formatVersion", "unsupported major; use a reader for this package's major version (this reader supports 1.x)"));
    }
    if minor.is_empty() || !minor.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("VERSION", "$.formatVersion", "invalid minor version"));
    }
    let minor = minor.parse::<u32>().map_err(|_| invalid("VERSION", "$.formatVersion", "minor version is too large"))?;
    let schema: Value = serde_json::from_slice(SCHEMA).map_err(|e| invalid("SCHEMA", "vpkg.schema.json", &e.to_string()))?;
    let mut checker = Checker { root: &schema, future: minor > 1, findings: Vec::new(), failed: false, work: 0, limits };
    checker.check(&raw, &schema, "$", 0)?;
    if checker.failed {
        return Err(Error::Invalid(checker.findings));
    }
    let mut warnings = checker.findings;
    if minor > 1 {
        warnings.push(finding(Severity::Warning, "VERSION", "$.formatVersion", "newer 1.x format; unknown fields and enum values were preserved"));
    }
    let data: Data = Data::deserialize(&raw).map_err(|e| invalid("MANIFEST", "vpkg.json", &e.to_string()))?;
    semantics(&data, limits)?;
    Ok(Manifest { raw, data, minor, warnings })
}

struct Checker<'a> {
    root: &'a Value,
    future: bool,
    findings: Vec<Diagnostic>,
    failed: bool,
    work: usize,
    limits: &'a Limits,
}
impl Checker<'_> {
    fn issue(&mut self, path: &str, message: &str, unknown: bool) {
        let severity = if unknown && self.future {
            Severity::Warning
        } else {
            self.failed = true;
            Severity::Error
        };
        if self.findings.len() < 128 {
            self.findings.push(finding(severity, "SCHEMA", path, message));
        }
    }
    fn check(&mut self, value: &Value, schema: &Value, path: &str, depth: usize) -> Result<(), Error> {
        self.work = self.work.saturating_add(1);
        if self.work > self.limits.max_work || depth > 64 {
            return Err(invalid("LIMIT", path, "schema validation work/depth budget exceeded"));
        }
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            let target = reference.strip_prefix('#').and_then(|r| self.root.pointer(r)).ok_or_else(|| invalid("SCHEMA", path, "unknown schema reference"))?;
            return self.check(value, target, path, depth + 1);
        }
        if let Some(kind) = schema.get("type").and_then(Value::as_str) {
            let ok = match kind {
                "object" => value.is_object(),
                "array" => value.is_array(),
                "string" => value.is_string(),
                "boolean" => value.is_boolean(),
                "number" => value.as_f64().is_some_and(f64::is_finite),
                "integer" => value.is_i64() || value.is_u64(),
                _ => false,
            };
            if !ok {
                self.issue(path, &format!("expected {kind}"), false);
                return Ok(());
            }
        }
        if schema.get("const").is_some_and(|c| c != value) {
            self.issue(path, "value differs from required constant", false);
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array)
            && !values.contains(value)
        {
            self.issue(path, "unknown enumeration value", true);
        }
        if let Some(text) = value.as_str() {
            if schema.get("minLength").and_then(Value::as_u64).is_some_and(|n| (text.chars().count() as u64) < n) {
                self.issue(path, "string is too short", false);
            }
            if let Some(pattern) = schema.get("pattern").and_then(Value::as_str)
                && !pattern_matches(pattern, text)?
            {
                self.issue(path, "string does not match the schema pattern", false);
            }
        }
        if let (Some(n), Some(min)) = (value.as_f64(), schema.get("minimum").and_then(Value::as_f64))
            && (!n.is_finite() || n < min)
        {
            self.issue(path, "number is below its minimum or non-finite", false);
        }
        if let Some(items) = value.as_array() {
            if schema.get("minItems").and_then(Value::as_u64).is_some_and(|n| (items.len() as u64) < n) {
                self.issue(path, "too few array items", false);
            }
            if let Some(item_schema) = schema.get("items") {
                for (i, item) in items.iter().enumerate() {
                    self.check(item, item_schema, &format!("{path}[{i}]"), depth + 1)?;
                }
            }
        }
        if let Some(object) = value.as_object() {
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for key in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(key) {
                        self.issue(path, &format!("missing required key {key}"), false);
                    }
                }
            }
            for (key, item) in object {
                if let Some(prop) = schema.get("properties").and_then(|p| p.get(key)) {
                    self.check(item, prop, &format!("{path}.{key}"), depth + 1)?;
                } else if let Some(extra) = schema.get("additionalProperties") {
                    if extra == &Value::Bool(false) {
                        self.issue(path, &format!("unknown key {}", key.chars().take(128).collect::<String>()), true);
                    } else if extra.is_object() {
                        self.check(item, extra, &format!("{path}.{key}"), depth + 1)?;
                    }
                }
            }
        }
        Ok(())
    }
}
fn pattern_matches(pattern: &str, text: &str) -> Result<bool, Error> {
    fn digits(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
    }
    let valid = match pattern {
        r"^1\.[0-9]+$" => text.strip_prefix("1.").is_some_and(digits),
        r"^[a-z0-9][a-z0-9._-]*$" => {
            text.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        }
        r"^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$" => {
            let (base, suffix) = text.split_once(['-', '+']).map_or((text, None), |(a, b)| (a, Some(b)));
            let mut parts = base.split('.');
            (0..3).all(|_| parts.next().is_some_and(digits))
                && parts.next().is_none()
                && suffix.is_none_or(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b)))
        }
        r"^(https?|file)://" => ["https://", "http://", "file://"].iter().any(|p| text.starts_with(p)),
        r"^(?!/)(?![A-Za-z]:)(?![\s\S]*(?:^|/)\.\.(?:/|$))[^\\\u0000-\u001f\u007f]+$" => {
            !text.is_empty()
                && !text.starts_with('/')
                && !text.contains('\\')
                && !text.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
                && !text.split('/').any(|p| p == "..")
                && !(text.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) && text.as_bytes().get(1) == Some(&b':'))
        }
        r"^[0-9a-f]{64}$" => text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f')),
        r"^[a-z][a-z0-9-]*$" => {
            text.bytes().next().is_some_and(|b| b.is_ascii_lowercase()) && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        }
        r"^[A-Za-z0-9][A-Za-z0-9_-]*$" => {
            text.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        }
        _ => return Err(invalid("SCHEMA", "vpkg.schema.json", "unsupported pattern in vendored schema")),
    };
    Ok(valid)
}
fn semantics(data: &Data, limits: &Limits) -> Result<(), Error> {
    if data.scenes.iter().filter(|s| s.role == "primary").count() != 1 {
        return Err(invalid("PRIMARY", "$.scenes", "exactly one primary scene is required"));
    }
    zip::unique_paths(data.scenes.iter().map(|s| s.path.as_str()), limits)?;
    zip::unique_paths(data.files.iter().map(|s| s.path.as_str()), limits)?;
    zip::unique_paths(data.fetch.iter().map(|s| s.path.as_str()), limits)?;
    let steps = &data.pipeline.steps;
    if steps.len() > limits.max_entries {
        return Err(invalid("LIMIT", "$.pipeline.steps", "too many pipeline steps"));
    }
    let mut ids = BTreeMap::new();
    for (i, s) in steps.iter().enumerate() {
        if ids.insert(s.id.as_str(), i).is_some() {
            return Err(invalid("PIPELINE", "$.pipeline.steps", "duplicate step id"));
        }
        if s.kind == "render" {
            if !s.render.as_ref().is_some_and(|r| data.scenes.iter().any(|scene| scene.path == r.scene)) {
                return Err(invalid("PIPELINE", &s.id, "render needs a block naming a listed scene"));
            }
        } else if s.run.is_none() {
            return Err(invalid("PIPELINE", &s.id, "non-render step needs run argv"));
        }
    }
    let mut degrees = vec![0usize; steps.len()];
    let mut outgoing = vec![Vec::new(); steps.len()];
    let mut edges = 0usize;
    for (i, s) in steps.iter().enumerate() {
        for need in &s.needs {
            edges = edges.saturating_add(1);
            if edges > limits.max_work {
                return Err(invalid("LIMIT", "$.pipeline.steps", "too many dependency edges"));
            }
            let &parent = ids.get(need.as_str()).ok_or_else(|| invalid("PIPELINE", &s.id, "needs an unknown step"))?;
            let degree = degrees.get_mut(i).ok_or_else(|| invalid("PIPELINE", &s.id, "invalid step index"))?;
            *degree = degree.saturating_add(1);
            outgoing.get_mut(parent).ok_or_else(|| invalid("PIPELINE", &s.id, "invalid parent index"))?.push(i);
        }
    }
    let mut ready: VecDeque<usize> = degrees.iter().enumerate().filter_map(|(i, &d)| (d == 0).then_some(i)).collect();
    let mut done = 0usize;
    while let Some(i) = ready.pop_front() {
        done = done.saturating_add(1);
        for &child in outgoing.get(i).into_iter().flatten() {
            let degree = degrees.get_mut(child).ok_or_else(|| invalid("PIPELINE", "$.pipeline.steps", "invalid child index"))?;
            *degree = degree.saturating_sub(1);
            if *degree == 0 {
                ready.push_back(child);
            }
        }
    }
    if done != steps.len() {
        return Err(invalid("PIPELINE", "$.pipeline.steps", "pipeline cycle"));
    }
    Ok(())
}
