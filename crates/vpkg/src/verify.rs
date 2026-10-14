//! Complete non-writing package verification.
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

use crate::{Diagnostic, Error, Limits, invalid, manifest::Manifest, zip::Entry};
use crate::{Severity, finding, manifest, zip};
use filmcraft_interchange::scene::read as scene_read;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// A completely verified package; payload bytes have one owner.
#[derive(Debug)]
pub struct Package {
    pub manifest: Manifest,
    pub primary_scene: String,
    pub entries: Vec<Entry>,
    pub warnings: Vec<Diagnostic>,
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    filmcraft_bitstream::sha256::to_hex(&filmcraft_bitstream::sha256::sha256(bytes))
}

/// Completely validate before returning any entries. No filesystem operations occur.
pub fn read(bytes: &[u8], limits: &Limits) -> Result<Package, Error> {
    let entries = zip::read(bytes, limits)?;
    for (entry, expected) in entries.iter().map(|e| e.path.as_str()).take(3).zip(["vpkg.json", "vpkg.schema.json", "README.md"]) {
        if entry != expected {
            return Err(invalid("ORDER", entry, "required order: vpkg.json, vpkg.schema.json, README.md"));
        }
    }
    if entries.len() < 3 {
        return Err(invalid("ORDER", "archive", "missing mandatory metadata entries"));
    }
    let allocated = entries
        .iter()
        .try_fold(bytes.len(), |sum, e| {
            sum.checked_add(e.bytes.capacity()).and_then(|n| n.checked_add(e.path.len().saturating_mul(8))).and_then(|n| n.checked_add(512))
        })
        .ok_or_else(|| invalid("LIMIT", "archive", "memory size overflow"))?;
    let mut remaining = *limits;
    remaining.max_memory_bytes =
        limits.max_memory_bytes.checked_sub(allocated).ok_or_else(|| invalid("LIMIT", "archive", "verification memory budget exceeded"))?;
    let manifest_bytes = &entries.first().ok_or_else(|| invalid("ORDER", "archive", "missing manifest"))?.bytes;
    let manifest = manifest::read(manifest_bytes, &remaining)?;
    remaining.max_memory_bytes = remaining
        .max_memory_bytes
        .checked_sub(manifest_bytes.len().saturating_mul(96))
        .ok_or_else(|| invalid("LIMIT", "vpkg.json", "manifest memory budget exceeded"))?;
    let mut findings = Findings::default();
    let by_name: BTreeMap<&str, &[u8]> = entries.iter().map(|e| (e.path.as_str(), e.bytes.as_slice())).collect();
    let expected: BTreeSet<String> = manifest.data.files.iter().map(|f| format!("project/{}", f.path)).collect();
    for entry in &entries {
        if !["vpkg.json", "vpkg.schema.json", "README.md"].contains(&entry.path.as_str()) && !expected.contains(&entry.path) {
            findings.error("STRAY", &entry.path, "archive entry is not listed in files[]");
        }
    }
    for file in &manifest.data.files {
        let name = format!("project/{}", file.path);
        match by_name.get(name.as_str()) {
            None => findings.error("MISSING", &file.path, "listed file is absent from archive"),
            Some(data) => {
                if data.len() as u64 != file.size {
                    findings.error("SIZE", &file.path, "size differs from files[]");
                }
                if hash(data) != file.sha256 {
                    findings.error("HASH", &file.path, "SHA-256 differs from files[]");
                }
            }
        }
    }
    match &manifest.data.packed.meta_sha256 {
        None if manifest.minor == 0 => {
            findings.warning("META_UNHASHED", "packed.metaSha256", "format 1.0: README.md and vpkg.schema.json are not hash-covered")
        }
        None => findings.error("META_HASH", "packed.metaSha256", "metadata hashes are mandatory from format 1.1"),
        Some(recorded) => {
            for name in ["README.md", "vpkg.schema.json"] {
                if !by_name.get(name).is_some_and(|bytes| recorded.get(name).is_some_and(|expected| hash(bytes) == *expected)) {
                    findings.error("META_HASH", name, "missing or mismatched metadata SHA-256");
                }
            }
        }
    }
    // Hash every listed entry even if structural manifest checks already found a problem.
    findings.finish()?;
    let project: BTreeMap<&str, &[u8]> = by_name.iter().filter_map(|(name, bytes)| name.strip_prefix("project/").map(|name| (name, *bytes))).collect();
    let mut warnings = manifest.warnings.clone();
    warnings.extend(findings.items);
    warnings.extend(check_references(&manifest, &project, &remaining, false)?.warnings);
    let primary_scene = manifest.data.scenes.iter().find(|s| s.role == "primary").ok_or_else(|| invalid("PRIMARY", "scenes", "no primary scene"))?.path.clone();
    Ok(Package { manifest, primary_scene, entries, warnings })
}

#[derive(Default)]
struct Findings {
    items: Vec<Diagnostic>,
    failed: bool,
}
impl Findings {
    fn push(&mut self, severity: Severity, code: &str, path: &str, message: &str) {
        self.failed |= severity == Severity::Error;
        if self.items.len() < 128 {
            self.items.push(finding(severity, code, path, message));
        } else if severity == Severity::Error && !self.items.iter().any(|d| d.severity == Severity::Error) {
            self.items.pop();
            self.items.push(finding(severity, code, path, message));
        }
    }
    fn error(&mut self, code: &str, path: &str, message: &str) {
        self.push(Severity::Error, code, path, message);
    }
    fn warning(&mut self, code: &str, path: &str, message: &str) {
        self.push(Severity::Warning, code, path, message);
    }
    fn finish(&self) -> Result<(), Error> {
        if self.failed { Err(Error::Invalid(self.items.clone())) } else { Ok(()) }
    }
}

pub(crate) fn xml_limits(limits: &Limits) -> scene_read::Limits {
    scene_read::Limits {
        max_bytes: limits.max_metadata_bytes.min(limits.max_memory_bytes / 128),
        max_nodes: u32::try_from(limits.max_work.min(limits.max_memory_bytes / 256)).unwrap_or(u32::MAX),
        max_depth: 64,
        max_attr_len: limits.max_name_bytes.max(4096).min(limits.max_metadata_bytes),
        max_text_len: limits.max_metadata_bytes,
        max_ids: limits.max_work,
    }
}
pub(crate) fn scene_doc<'a>(bytes: &'a [u8], limits: &Limits) -> Result<scene_read::SceneDoc<'a>, Error> {
    let bounds = xml_limits(limits);
    let text = scene_read::decode(bytes, &bounds).map_err(|e| invalid("XML", "scene", &e.to_string()))?;
    scene_read::read(text, &bounds).map_err(|e| invalid("XML", "scene", &e.to_string()))
}

fn fragment_text<'a>(bytes: &'a [u8], bounds: &scene_read::Limits, path: &str) -> Result<&'a str, Error> {
    let text = scene_read::decode(bytes, bounds).map_err(|e| invalid("XML", path, &e.to_string()))?.trim_start();
    if text.starts_with("<?xml") && matches!(text.as_bytes().get(5), Some(b' ' | b'\t' | b'\r' | b'\n')) {
        text.split_once("?>").map(|(_, body)| body).ok_or_else(|| invalid("XML", path, "unterminated XML declaration"))
    } else {
        Ok(text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Scene,
    Include,
    Asset,
}
struct Scan<'a> {
    files: &'a BTreeMap<&'a str, &'a [u8]>,
    manifest: &'a Manifest,
    limits: &'a Limits,
    strict_remote: bool,
    findings: Findings,
    work: usize,
    queued: BTreeSet<(String, Kind)>,
    queue: VecDeque<(String, Kind)>,
    families: BTreeSet<String>,
    declared: BTreeSet<String>,
    audio: bool,
}
impl Scan<'_> {
    fn tick(&mut self, n: usize) -> Result<(), Error> {
        self.work = self.work.saturating_add(n);
        if self.work > self.limits.max_work { Err(invalid("LIMIT", "references", "reference work budget exceeded")) } else { Ok(()) }
    }
    fn enqueue(&mut self, path: &str, kind: Kind) -> Result<(), Error> {
        self.tick(1)?;
        if self.queued.insert((path.to_owned(), kind)) {
            if self.queued.len() > self.limits.max_entries.saturating_mul(3) {
                return Err(invalid("LIMIT", path, "dependency expansion budget exceeded"));
            }
            self.queue.push_back((path.to_owned(), kind));
        }
        Ok(())
    }
    fn generated(&mut self, path: &str) -> Result<bool, Error> {
        for step in &self.manifest.data.pipeline.steps {
            for pattern in step.outputs.iter().map(String::as_str).chain(step.render.as_ref().and_then(|r| r.to.as_deref())) {
                self.tick(1)?;
                if path == pattern || glob_matches(path, pattern, &mut self.work, self.limits.max_work)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn permitted(&mut self, path: &str) -> Result<bool, Error> {
        for f in &self.manifest.data.fetch {
            self.tick(1)?;
            if f.path == path {
                return Ok(true);
            }
        }
        self.generated(path)
    }
    fn reference(&mut self, source: &str, value: &str, kind: Kind, sequence: bool, optional: bool) -> Result<(), Error> {
        self.tick(1)?;
        let Some(path) = resolve(source, value, sequence, self.limits)? else {
            return Ok(());
        };
        let path = match path {
            Reference::Local(path) => path,
            Reference::Remote => {
                if self.strict_remote {
                    self.findings.error("REMOTE", source, "remote input is not portable; supply a packaged local input");
                } else {
                    self.findings.warning("REMOTE", source, "remote input is not verified or fetched");
                }
                return Ok(());
            }
        };
        if sequence && let Some(tokens) = sequence_tokens(&path)? {
            let mut found = false;
            for &candidate in self.files.keys() {
                if tokens_match(candidate, &tokens, &mut self.work, self.limits.max_work)? {
                    found = true;
                    self.enqueue(candidate, Kind::Asset)?;
                }
            }
            if !found && !optional && !self.permitted(&path)? {
                self.findings.error("REFERENCE", source, "image sequence has no packaged frames or declared producer");
            }
            return Ok(());
        }
        if self.files.contains_key(path.as_str()) {
            self.enqueue(&path, kind)?;
        } else if !optional && !self.permitted(&path)? {
            self.findings.error("REFERENCE", source, &format!("missing input: {path}"));
        }
        Ok(())
    }
    fn xml(&mut self, source: &str, doc: &scene_read::SceneDoc<'_>) -> Result<(), Error> {
        for el in doc.doc.descendants().filter(|n| n.is_element()) {
            self.tick(1)?;
            let tag = el.tag_name().name();
            self.audio |= tag == "audio" && el.attribute("src").is_some_and(|s| !s.is_empty());
            if tag == "font" {
                if let Some(f) = el.attribute("family") {
                    self.declared.insert(f.to_uppercase());
                }
            } else {
                for attr in ["font", "fallback"] {
                    if let Some(list) = el.attribute(attr) {
                        for family in list.split(',') {
                            self.tick(1)?;
                            let family = family.trim().trim_matches(['\'', '"']);
                            if !family.is_empty() {
                                self.families.insert(family.to_uppercase());
                            }
                        }
                    }
                }
            }
            for attr in el.attributes() {
                let name = attr.name();
                let cache = name == "cache" && matches!(tag, "captionTrack" | "generated" | "physics");
                if input_attribute(tag, name) || cache {
                    self.reference(source, attr.value(), if tag == "include" { Kind::Include } else { Kind::Asset }, tag == "imageSequence", cache)?;
                }
            }
        }
        Ok(())
    }
    fn included(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        let bounds = xml_limits(self.limits);
        let text = fragment_text(bytes, &bounds, path)?;
        // Includes can have <assets> or other fragment roots; wrapping reuses the bounded scene parser.
        let wrapped = format!("<scene version=\"1.7\">{text}</scene>");
        let doc = scene_read::read(&wrapped, &bounds).map_err(|e| invalid("XML", path, &e.to_string()))?;
        self.xml(path, &doc)
    }
    fn dependency(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !["gltf", "glb", "obj", "mtl", "mtlx"].contains(&ext.as_str()) {
            return Ok(());
        }
        if bytes.len() > self.limits.max_metadata_bytes || bytes.len().saturating_mul(128) > self.limits.max_memory_bytes {
            return Err(invalid("LIMIT", path, "nested dependency document exceeds budget"));
        }
        match ext.as_str() {
            "gltf" | "glb" => {
                let json = if ext == "glb" {
                    let head = bytes.get(..20).ok_or_else(|| invalid("DEPENDENCY", path, "truncated GLB"))?;
                    if head.get(..4) != Some(b"glTF") || head.get(12..16).is_none() || head.get(16..20) != Some(b"JSON") {
                        return Err(invalid("DEPENDENCY", path, "GLB has no first JSON chunk"));
                    }
                    let word = |at: usize| -> Result<u32, Error> {
                        let raw: [u8; 4] = head
                            .get(at..at.saturating_add(4))
                            .and_then(|s| s.try_into().ok())
                            .ok_or_else(|| invalid("DEPENDENCY", path, "truncated GLB field"))?;
                        Ok(u32::from_le_bytes(raw))
                    };
                    let size = usize::try_from(word(12)?).map_err(|_| invalid("DEPENDENCY", path, "GLB chunk too large"))?;
                    if word(4)? != 2 || u64::from(word(8)?) != bytes.len() as u64 {
                        return Err(invalid("DEPENDENCY", path, "invalid GLB version or length"));
                    }
                    bytes
                        .get(20..20usize.checked_add(size).ok_or_else(|| invalid("DEPENDENCY", path, "GLB size overflow"))?)
                        .ok_or_else(|| invalid("DEPENDENCY", path, "truncated GLB JSON chunk"))?
                } else {
                    bytes
                };
                let doc: Value = serde_json::from_slice(json).map_err(|e| invalid("DEPENDENCY", path, &e.to_string()))?;
                for list in ["buffers", "images"] {
                    if let Some(items) = doc.get(list).and_then(Value::as_array) {
                        for item in items {
                            self.tick(1)?;
                            if let Some(uri) = item.get("uri").and_then(Value::as_str) {
                                self.reference(path, uri, Kind::Asset, false, false)?;
                            }
                        }
                    }
                }
            }
            "obj" | "mtl" => {
                let text = std::str::from_utf8(bytes).map_err(|_| invalid("DEPENDENCY", path, "asset reference document is not UTF-8"))?;
                for line in text.lines() {
                    self.tick(1)?;
                    let value = if ext == "obj" {
                        line.strip_prefix("mtllib ").map(str::trim)
                    } else {
                        let trimmed = line.trim();
                        let low = trimmed.to_ascii_lowercase();
                        if ["map_", "bump", "disp", "norm"].iter().any(|p| low.starts_with(p)) && trimmed.split_whitespace().nth(1).is_some() {
                            trimmed.split_whitespace().last()
                        } else {
                            None
                        }
                    };
                    if let Some(value) = value {
                        self.reference(path, value, Kind::Asset, false, false)?;
                    }
                }
            }
            "mtlx" => {
                let bounds = xml_limits(self.limits);
                let text = fragment_text(bytes, &bounds, path)?;
                let wrapped = format!("<scene version=\"1.7\">{text}</scene>");
                let doc = scene_read::read(&wrapped, &bounds).map_err(|e| invalid("XML", path, &e.to_string()))?;
                for el in doc.doc.descendants().filter(|n| n.is_element()) {
                    self.tick(1)?;
                    if el.attribute("type") == Some("filename")
                        && let Some(value) = el.attribute("value")
                    {
                        self.reference(path, value, Kind::Asset, false, false)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub(crate) struct ReferenceFacts {
    pub warnings: Vec<Diagnostic>,
    pub audio: bool,
}

pub(crate) fn check_references(manifest: &Manifest, files: &BTreeMap<&str, &[u8]>, limits: &Limits, strict_remote: bool) -> Result<ReferenceFacts, Error> {
    let mut scan = Scan {
        files,
        manifest,
        limits,
        strict_remote,
        findings: Findings::default(),
        work: 0,
        queued: BTreeSet::new(),
        queue: VecDeque::new(),
        families: BTreeSet::new(),
        declared: BTreeSet::new(),
        audio: false,
    };
    for scene in &manifest.data.scenes {
        if !matches!(scene.role.as_str(), "primary" | "variant" | "archive") {
            continue;
        }
        if files.contains_key(scene.path.as_str()) {
            scan.enqueue(&scene.path, Kind::Scene)?;
        } else if scene.role != "archive" && !scan.permitted(&scene.path)? {
            scan.findings.error("REFERENCE", &scene.path, "listed scene is neither packaged nor supplied by fetch/pipeline");
        }
    }
    while let Some((path, kind)) = scan.queue.pop_front() {
        let bytes = files.get(path.as_str()).ok_or_else(|| invalid("REFERENCE", &path, "queued file is missing"))?;
        match kind {
            Kind::Scene => {
                let doc = scene_doc(bytes, limits)?;
                scan.xml(&path, &doc)?;
            }
            Kind::Include => scan.included(&path, bytes)?,
            Kind::Asset => scan.dependency(&path, bytes)?,
        }
    }
    for family in &scan.families {
        if !scan.declared.contains(family) && !["SERIF", "SANS-SERIF", "MONOSPACE", "CURSIVE", "FANTASY", "SYSTEM-UI"].contains(&family.as_str()) {
            scan.findings.warning("FONT", "scenes", "undeclared font family may resolve from the host; this crate does not resolve fonts");
            break;
        }
    }
    scan.findings.finish()?;
    Ok(ReferenceFacts { warnings: scan.findings.items, audio: scan.audio })
}

fn input_attribute(tag: &str, attr: &str) -> bool {
    match attr {
        "src" => matches!(
            tag,
            "audio"
                | "captionTrack"
                | "chart"
                | "data"
                | "effect"
                | "font"
                | "image"
                | "imageSequence"
                | "include"
                | "look"
                | "lottie"
                | "mesh"
                | "representation"
                | "trackData"
                | "vector"
                | "video"
        ),
        "proxy" => matches!(tag, "audio" | "font" | "image" | "imageSequence" | "lottie" | "mesh" | "vector" | "video"),
        "fontFile" => matches!(tag, "span" | "text" | "textStyle"),
        "environment" | "ies" => tag == "light",
        "ocioConfig" => tag == "colorManagement",
        "shader" => tag == "transition",
        "weights" => tag == "skeleton",
        "baseColorMap" | "displacementMap" | "emissiveMap" | "materialX" | "metallicRoughnessMap" | "normalMap" | "occlusionMap" => tag == "material",
        _ => false,
    }
}
enum Reference {
    Local(String),
    Remote,
}
fn resolve(source: &str, value: &str, sequence: bool, limits: &Limits) -> Result<Option<Reference>, Error> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.split_once(':').is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("data")) {
        return Ok(None);
    }
    if value.len() > limits.max_name_bytes {
        return Err(invalid("LIMIT", source, "reference path exceeds name budget"));
    }
    if let Some((scheme, _)) = value.split_once(':') {
        if scheme.eq_ignore_ascii_case("data") {
            return Ok(None);
        }
        if scheme.len() == 1 || scheme.eq_ignore_ascii_case("file") {
            return Err(invalid("PATH", source, "drive letters and file URIs are not portable"));
        }
        if scheme.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()) && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b)) {
            return Ok(Some(Reference::Remote));
        }
    }
    let path = if sequence {
        value.to_owned()
    } else {
        let plain = value.split(['#', '?']).next().unwrap_or("");
        let mut out = Vec::new();
        let mut iter = plain.bytes();
        while let Some(b) = iter.next() {
            if b == b'%' {
                let hex = |c: u8| -> Option<u8> {
                    match c {
                        b'0'..=b'9' => Some(c - b'0'),
                        b'a'..=b'f' => Some(c - b'a' + 10),
                        b'A'..=b'F' => Some(c - b'A' + 10),
                        _ => None,
                    }
                };
                let a = iter.next().and_then(hex).ok_or_else(|| invalid("PATH", source, "invalid percent escape"))?;
                let b = iter.next().and_then(hex).ok_or_else(|| invalid("PATH", source, "invalid percent escape"))?;
                out.push(a * 16 + b);
            } else {
                out.push(b);
            }
        }
        String::from_utf8(out).map_err(|_| invalid("PATH", source, "reference is not UTF-8"))?
    };
    if path.starts_with('/') || path.contains(['\\', ':']) || path.chars().any(char::is_control) {
        return Err(invalid("PATH", source, "unsafe input reference"));
    }
    let base = source.rsplit_once('/').map_or("", |(base, _)| base);
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(invalid("PATH", source, "input escapes project root"));
                }
            }
            _ => parts.push(p),
        }
    }
    let joined = parts.join("/");
    if joined.len() > limits.max_name_bytes || !zip::safe_path(&joined) {
        return Err(invalid("PATH", source, "invalid resolved input path"));
    }
    Ok(Some(Reference::Local(joined)))
}

#[derive(Clone, Copy)]
enum Token {
    Char(char),
    One,
    Star(bool),
    Directories,
    Digits(usize, bool),
}
fn sequence_tokens(path: &str) -> Result<Option<Vec<Token>>, Error> {
    let (prefix, basename) = path.rsplit_once('/').map_or(("", path), |(base, name)| (base, name));
    let mut tokens: Vec<Token> = prefix.chars().map(Token::Char).collect();
    if !prefix.is_empty() {
        tokens.push(Token::Char('/'));
    }
    let mut chars = basename.chars().peekable();
    let mut pattern = false;
    while let Some(c) = chars.next() {
        if c == '#' {
            let mut n = 1usize;
            while chars.peek() == Some(&'#') {
                chars.next();
                n = n.saturating_add(1);
            }
            tokens.push(Token::Digits(n, true));
            pattern = true;
        } else if c == '%' {
            let mut probe = chars.clone();
            let mut n = 0usize;
            let mut digits = 0usize;
            while let Some(&b) = probe.peek().filter(|b| b.is_ascii_digit()) {
                probe.next();
                n = n
                    .checked_mul(10)
                    .and_then(|n| n.checked_add(usize::from(b as u8 - b'0')))
                    .ok_or_else(|| invalid("LIMIT", path, "image sequence width too large"))?;
                digits += 1;
            }
            if probe.next() == Some('d') {
                if n > path.len().max(1024) {
                    return Err(invalid("LIMIT", path, "image sequence width too large"));
                }
                for _ in 0..digits.saturating_add(1) {
                    chars.next();
                }
                tokens.push(Token::Digits(n.max(1), false));
                pattern = true;
            } else {
                tokens.push(Token::Char(c));
            }
        } else {
            tokens.push(Token::Char(c));
        }
    }
    Ok(pattern.then_some(tokens))
}
fn glob_matches(path: &str, pattern: &str, work: &mut usize, limit: usize) -> Result<bool, Error> {
    if pattern.len() > 4096 {
        return Err(invalid("LIMIT", "pipeline", "output glob too long"));
    }
    let anchored = pattern.trim_end_matches('/').contains('/');
    let trailing = pattern.ends_with('/');
    let pattern = pattern.trim_start_matches('/');
    let mut tokens = Vec::new();
    if !anchored {
        tokens.push(Token::Directories);
    }
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    if chars.peek() == Some(&'/') {
                        chars.next();
                        tokens.push(Token::Directories);
                    } else {
                        tokens.push(Token::Star(true));
                    }
                } else {
                    tokens.push(Token::Star(false));
                }
            }
            '?' => tokens.push(Token::One),
            '{' => {
                let mut probe = chars.clone();
                let mut count = 0usize;
                let mut closed = false;
                for c in probe.by_ref() {
                    count += 1;
                    if c == '}' {
                        closed = true;
                        break;
                    }
                }
                if closed {
                    for _ in 0..count {
                        chars.next();
                    }
                    tokens.push(Token::Star(false));
                } else {
                    tokens.push(Token::Char(c));
                }
            }
            _ => tokens.push(Token::Char(c)),
        }
    }
    if trailing {
        tokens.push(Token::Star(true));
    }
    tokens_match(path, &tokens, work, limit)
}
fn tokens_match(path: &str, tokens: &[Token], work: &mut usize, limit: usize) -> Result<bool, Error> {
    let characters: Vec<char> = path.chars().collect();
    let mut current = vec![false; characters.len().saturating_add(1)];
    if let Some(v) = current.first_mut() {
        *v = true;
    }
    for token in tokens {
        let mut next = vec![false; current.len()];
        let mut active = false;
        for (i, &matches) in current.iter().enumerate() {
            *work = work.saturating_add(1);
            if *work > limit {
                return Err(invalid("LIMIT", "references", "glob matching budget exceeded"));
            }
            let byte = characters.get(i).copied();
            let mut set = |index: usize| {
                if let Some(n) = next.get_mut(index) {
                    *n = true;
                }
            };
            match *token {
                Token::Char(want) => {
                    if matches && byte == Some(want) {
                        set(i.saturating_add(1));
                    }
                }
                Token::One => {
                    if matches && byte.is_some_and(|b| b != '/') {
                        set(i.saturating_add(1));
                    }
                }
                Token::Star(cross) => {
                    active |= matches;
                    if active {
                        set(i);
                        if !cross && byte == Some('/') {
                            active = false;
                        }
                    }
                }
                Token::Directories => {
                    active |= matches;
                    if matches {
                        set(i);
                    }
                    if active && byte == Some('/') {
                        set(i.saturating_add(1));
                    }
                }
                Token::Digits(min, exact) => {
                    if matches {
                        let mut count = 0usize;
                        for b in characters.get(i..).unwrap_or(&[]) {
                            *work = work.saturating_add(1);
                            if *work > limit {
                                return Err(invalid("LIMIT", "references", "sequence matching budget exceeded"));
                            }
                            if !b.is_ascii_digit() {
                                break;
                            }
                            count = count.saturating_add(1);
                            if count >= min {
                                set(i.saturating_add(count));
                                if exact {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        current = next;
    }
    Ok(current.last().copied().unwrap_or(false))
}
