//! Scene video package commands through host services.
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

use crate::{EngineError, Result, Session};
use filmcraft_interchange::{ExportOptions, Format};
use filmcraft_project::ItemId;
use filmcraft_vpkg::{FileInput, Limits, PackInput, manifest::Metadata};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const IMPORT: &str = "file.importVpkg";
const EXPORT: &str = "file.exportVpkg";

fn params(p: &Value, command: &str, allowed: &[&str]) -> Result<()> {
    let object = p.as_object().ok_or_else(|| crate::commands::bad(command, "expected an object"))?;
    if object.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(crate::commands::bad(command, "unknown parameter"));
    }
    Ok(())
}
fn string<'a>(p: &'a Value, key: &str, command: &str) -> Result<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 16 << 10 && !s.chars().any(char::is_control))
        .ok_or_else(|| crate::commands::bad(command, format!("`{key}` must be a nonempty string of at most 16 KiB without control characters")))
}
fn path<'a>(p: &'a Value, key: &str, command: &str) -> Result<&'a str> {
    let value = string(p, key, command)?;
    if value.split(['/', '\\']).any(|p| p == "..") {
        return Err(crate::commands::bad(command, format!("`{key}` must not contain parent components")));
    }
    Ok(value)
}
fn io(path: &str, error: std::io::Error) -> EngineError {
    EngineError::Other(format!("{path}: {error}"))
}
fn invalid(path: &str, error: filmcraft_vpkg::Error) -> EngineError {
    let message = format!("{path}: {error}");
    let filmcraft_vpkg::Error::Invalid(diagnostics) = error;
    EngineError::InvalidPackage { message, diagnostics }
}
fn read(s: &Session, path: &str, max: usize, command: &str) -> Result<Vec<u8>> {
    let size = s.services.file_size(path).map_err(|e| io(path, e))?;
    if size > max as u64 {
        return Err(crate::commands::bad(command, format!("{path}: file exceeds the {max}-byte package budget")));
    }
    s.services.read_file_bounded(path, max).map_err(|e| io(path, e))
}
fn default_dir(path: &str) -> Result<String> {
    let file = Path::new(path);
    let name = file.file_name().and_then(|s| s.to_str()).ok_or_else(|| crate::commands::bad(IMPORT, "package path needs a filename"))?;
    let stem =
        if name.to_ascii_lowercase().ends_with(".vpkg.zip") { name.get(..name.len().saturating_sub(9)) } else { file.file_stem().and_then(|s| s.to_str()) }
            .filter(|s| !s.is_empty() && *s != "." && *s != "..")
            .ok_or_else(|| crate::commands::bad(IMPORT, "package path needs a nonempty stem"))?;
    Ok(file.with_file_name(stem).to_string_lossy().into_owned())
}

/// Verify and extract a package, then import its primary scene with the scene command's modes.
pub fn import(s: &mut Session, p: &Value) -> Result<Value> {
    params(p, IMPORT, &["path", "dir", "mode", "lenient"])?;
    let source = path(p, "path", IMPORT)?;
    let dir = if p.get("dir").is_some() { path(p, "dir", IMPORT)?.to_string() } else { default_dir(source)? };
    let mode = match p.get("mode") {
        None => "hybrid",
        Some(Value::String(mode)) if matches!(mode.as_str(), "rendered" | "native" | "hybrid") => mode,
        _ => return Err(crate::commands::bad(IMPORT, "`mode` must be rendered, native or hybrid")),
    };
    let lenient = crate::interchange::lenient_param(p, IMPORT)?;
    let limits = Limits::default();
    let bytes = read(s, source, limits.max_memory_bytes, IMPORT)?;
    let package = filmcraft_vpkg::read(&bytes, &limits).map_err(|e| invalid(source, e))?;
    drop(bytes);
    if p.get("dir").is_none() && !s.services.package_dir_empty(&dir).map_err(|e| io(&dir, e))? {
        return Err(EngineError::Other(format!("{dir}: default extraction directory already exists and is not empty; supply an explicit dir")));
    }
    let primary = format!("project/{}", package.primary_scene);
    if !package.entries.iter().any(|entry| entry.path == primary) {
        return Err(EngineError::Other(format!("{}: primary scene is not packaged; fetches and pipelines are not executed", package.primary_scene)));
    }
    let scene = s.services.package_path(&dir, &primary).map_err(|e| io(&primary, e))?;
    let root = Path::new(&scene)
        .ancestors()
        .nth(primary.split('/').count())
        .and_then(Path::to_str)
        .ok_or_else(|| EngineError::Other("invalid extraction root".into()))?;
    let mut documents = normalize_graph(s, root, &primary, &package)?;
    let normalized = documents.remove(&primary).ok_or_else(|| EngineError::Other("missing normalized primary scene".into()))?;
    let mut generated: Vec<_> = documents.into_iter().map(|(source, bytes)| (format!("{source}.filmcraft.scene"), bytes)).collect();
    let prepared = if mode == "rendered" {
        None
    } else {
        let mut prepared = crate::interchange::prepare_scene(s, &scene, &normalized, mode, lenient)?;
        for overlay in &mut prepared.overlays {
            let relative = relative_destination(s, root, &overlay.path)?;
            // The pure importer emits filesystem paths. Encode them for the renderer once.
            overlay.document = rewrite_references(&overlay.document, |_, _, value, sequence| {
                if value.contains("://") && !value.starts_with("file://") {
                    return Err(EngineError::Other(format!("unverified overlay input: {value}")));
                }
                let base = Path::new(&overlay.path).parent().and_then(Path::to_str);
                let target = filmcraft_interchange::resolve_path(value, base);
                relative_destination(s, root, &target)?;
                Ok(renderer_reference(&target, value, sequence))
            })?;
            generated.push((relative, std::mem::take(&mut overlay.document)));
        }
        // No media open may occur until all resulting file paths have passed containment.
        for item in prepared.fragment.project.items.values() {
            if let Some(media) = item.as_media()
                && let filmcraft_project::MediaRef::File { path } = &media.media
            {
                relative_destination(s, root, path)?;
            }
        }
        Some(prepared)
    };
    let derived = format!("{primary}.filmcraft.scene");
    let rendered_path = s.services.package_path(root, &derived).map_err(|e| io(&derived, e))?;
    generated.push((derived, normalized));
    // Validate archive and generated destinations before writing even the first metadata file.
    let mut destinations = BTreeMap::new();
    for (relative, bytes) in
        package.entries.iter().map(|e| (e.path.as_str(), e.bytes.as_slice())).chain(generated.iter().map(|(path, bytes)| (path.as_str(), bytes.as_slice())))
    {
        if destinations.insert(relative, bytes).is_some_and(|existing| existing != bytes) {
            return Err(EngineError::Other(format!("{relative}: generated scene conflicts with a package entry")));
        }
    }
    if destinations.len() > limits.max_entries
        || destinations.values().try_fold(0_usize, |total, bytes| total.checked_add(bytes.len())).is_none_or(|total| total > limits.max_total_bytes)
    {
        return Err(EngineError::Other("archive and generated files exceed extraction budget".into()));
    }
    validate_destinations(&destinations)?;
    for (relative, bytes) in &destinations {
        preflight(s, root, relative, bytes)?;
    }
    let mut retained = Vec::new();
    for (relative, bytes) in
        package.entries.iter().map(|e| (e.path.as_str(), e.bytes.as_slice())).chain(generated.iter().map(|(path, bytes)| (path.as_str(), bytes.as_slice())))
    {
        if let Err(error) = s.services.write_package_file(root, relative, bytes) {
            return Err(EngineError::Other(format!(
                "extraction into {root} failed at {relative}: {error}; retained files: {}; retry with explicit dir after resolving the error",
                if retained.is_empty() { "none".into() } else { retained.join(", ") }
            )));
        }
        retained.push(relative);
    }
    let mut result = s.edit_group("Import Scene Package", |s| {
        if let Some(prepared) = prepared {
            crate::interchange::finish_scene(s, prepared)
        } else {
            crate::interchange::import_scene(s, &json!({"path":rendered_path,"mode":mode,"lenient":lenient}))
        }
    })?;
    let metadata = &package.manifest.data.package;
    if let Some(object) = result.as_object_mut() {
        object.insert(
            "package".into(),
            json!({"id":metadata.id,"version":metadata.version,"title":metadata.title,
            "primaryScene":package.primary_scene,"extractedTo":dir,"warnings":package.warnings}),
        );
    }
    Ok(result)
}

fn validate_destinations(destinations: &BTreeMap<&str, &[u8]>) -> Result<()> {
    let mut files = BTreeMap::new();
    let mut directories = BTreeSet::new();
    for relative in destinations.keys() {
        // Match the package verifier's portable case folding, including expanding Unicode pairs.
        let folded = relative.to_lowercase().to_uppercase();
        if let Some(previous) = files.insert(folded.clone(), *relative) {
            return Err(EngineError::Other(format!("{relative}: platform-equivalent destination conflicts with {previous}")));
        }
        let mut parent = folded.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            directories.insert(prefix.to_string());
            parent = prefix;
        }
    }
    for directory in directories {
        if let Some(file) = files.get(&directory) {
            return Err(EngineError::Other(format!("{file}: destination is both a file and a directory")));
        }
    }
    Ok(())
}

fn preflight(s: &Session, root: &str, relative: &str, bytes: &[u8]) -> Result<()> {
    let target = s.services.package_path(root, relative).map_err(|e| io(relative, e))?;
    match s.services.file_size(&target) {
        Ok(size) => {
            if size != bytes.len() as u64 || s.services.read_file_bounded(&target, bytes.len()).map_err(|e| io(&target, e))? != bytes {
                return Err(EngineError::Other(format!("{target}: file already exists with different content")));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io(&target, e)),
    }
    Ok(())
}

fn relative_destination(s: &Session, root: &str, target: &str) -> Result<String> {
    let relative = relative_path(root, target).ok_or_else(|| EngineError::Other(format!("{target}: scene path escapes extraction directory {root}")))?;
    s.services.package_path(root, &relative).map_err(|e| io(target, e))?;
    Ok(relative)
}

// Input classification and URI policy adapted from sr-core sr_core/vpkg/refs.py
// (_value_path and INPUTS/CACHES), MIT; see ATTRIBUTION.md and crates/interchange/schema/sr-core/LICENSE.
pub(crate) fn input_attribute(tag: &str, attr: &str) -> bool {
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
                | "strokeFont"
                | "geo"
                | "pdf"
                | "tiles"
                | "compute"
                | "volume"
                | "voxelAsset"
                | "meshSequence"
                | "program"
        ),
        "proxy" => matches!(
            tag,
            "audio"
                | "font"
                | "image"
                | "imageSequence"
                | "lottie"
                | "mesh"
                | "vector"
                | "video"
                | "strokeFont"
                | "geo"
                | "pdf"
                | "voxelAsset"
                | "meshSequence"
        ),
        "fontFile" => matches!(tag, "span" | "text" | "textStyle"),
        "environment" | "ies" => tag == "light",
        "ocioConfig" => tag == "colorManagement",
        "shader" => tag == "transition",
        "weights" => tag == "skeleton",
        "baseColorMap" | "displacementMap" | "emissiveMap" | "materialX" | "metallicRoughnessMap" | "normalMap" | "occlusionMap" => tag == "material",
        "cache" => matches!(tag, "captionTrack" | "generated" | "physics" | "tiles" | "pdf"),
        "mapStyle" => tag == "basemap",
        "model" => tag == "captionTrack",
        _ => false,
    }
}

fn local_reference(value: &str, sequence: bool) -> Result<String> {
    if sequence {
        return Ok(value.into());
    }
    let plain = value.split(['#', '?']).next().unwrap_or("");
    let mut bytes = Vec::new();
    let mut iter = plain.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let hex = |byte: u8| char::from(byte).to_digit(16).and_then(|v| u8::try_from(v).ok());
            let high = iter.next().and_then(hex).ok_or_else(|| EngineError::Other("invalid package URI escape".into()))?;
            let low = iter.next().and_then(hex).ok_or_else(|| EngineError::Other("invalid package URI escape".into()))?;
            bytes.push(high * 16 + low);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|e| EngineError::Other(format!("invalid UTF-8 package URI: {e}")))
}

fn renderer_reference(target: &str, relative: &str, sequence: bool) -> String {
    if Path::new(target).is_absolute() {
        if sequence { target.into() } else { filmcraft_interchange::path_to_file_url(target, false) }
    } else {
        relative.into()
    }
}

struct ReferenceGraph<'a> {
    session: &'a Session,
    root: &'a str,
    files: BTreeMap<&'a str, &'a [u8]>,
    seen: BTreeSet<(String, bool)>,
    pending: Vec<(String, bool)>,
    work: usize,
    generated_bytes: usize,
}
impl ReferenceGraph<'_> {
    fn charge(&mut self, amount: usize) -> Result<()> {
        self.work = self
            .work
            .checked_add(amount)
            .filter(|work| *work <= Limits::default().max_work)
            .ok_or_else(|| EngineError::Other("package dependency graph exceeds work budget".into()))?;
        Ok(())
    }
    fn enqueue(&mut self, path: &str, document: bool) -> Result<()> {
        self.charge(1)?;
        if self.seen.len() >= Limits::default().max_entries.saturating_mul(2) {
            return Err(EngineError::Other("too many package dependencies".into()));
        }
        let key = (path.to_string(), document);
        if self.seen.insert(key.clone()) {
            self.pending.push(key);
        }
        Ok(())
    }
    fn reference(&mut self, source: &str, value: &str, document: bool, sequence: bool) -> Result<String> {
        self.charge(1)?;
        let relative = resolve_reference(source, value, sequence)?;
        let target = self.session.services.package_path(self.root, &relative).map_err(|e| io(value, e))?;
        if self.files.contains_key(relative.as_str()) {
            self.enqueue(&relative, document)?;
        } else if document {
            return Err(EngineError::Other(format!("{relative}: included document is not packaged; cannot inspect its dependencies")));
        } else {
            // A fetch/producer declaration does not verify a pre-existing local file.
            match self.session.services.file_size(&target) {
                Ok(_) => return Err(EngineError::Other(format!("{relative}: existing input has no verified package bytes"))),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io(&target, error)),
            }
        }
        Ok(relative)
    }
    fn normalize(&mut self, source: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        self.charge(bytes.len())?;
        rewrite_references(bytes, |tag, _, value, sequence| {
            let document = tag == "include";
            let relative = self.reference(source, value, document, sequence)?;
            let destination = if document { format!("{relative}.filmcraft.scene") } else { relative };
            let target = self.session.services.package_path(self.root, &destination).map_err(|e| io(&destination, e))?;
            let relative_value = local_reference(value, sequence)?;
            let relative_value = if document { format!("{relative_value}.filmcraft.scene") } else { relative_value };
            Ok(renderer_reference(&target, &relative_value, sequence))
        })
    }
    fn dependency(&mut self, source: &str, bytes: &[u8]) -> Result<()> {
        let extension = source.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if matches!(extension.as_str(), "usd" | "usda" | "usdc" | "usdz" | "fbx" | "svg" | "srvseq" | "ocio") {
            return Err(EngineError::Other(format!("{source}: package dependency format cannot be safely inspected")));
        }
        if !matches!(extension.as_str(), "gltf" | "glb" | "obj" | "mtl" | "mtlx") {
            return Ok(());
        }
        if bytes.len() > Limits::default().max_metadata_bytes {
            return Err(EngineError::Other(format!("{source}: dependency document exceeds byte budget")));
        }
        self.charge(bytes.len())?;
        match extension.as_str() {
            "gltf" | "glb" => {
                let json = if extension == "glb" {
                    let size = bytes
                        .get(12..16)
                        .and_then(|raw| <[u8; 4]>::try_from(raw).ok())
                        .map(u32::from_le_bytes)
                        .and_then(|n| usize::try_from(n).ok())
                        .ok_or_else(|| EngineError::Other(format!("{source}: truncated GLB")))?;
                    if bytes.get(..4) != Some(b"glTF") || bytes.get(16..20) != Some(b"JSON") {
                        return Err(EngineError::Other(format!("{source}: GLB has no first JSON chunk")));
                    }
                    bytes.get(20..20_usize.saturating_add(size)).ok_or_else(|| EngineError::Other(format!("{source}: truncated GLB JSON")))?
                } else {
                    bytes
                };
                let doc: Value = serde_json::from_slice(json).map_err(|e| EngineError::Other(format!("{source}: {e}")))?;
                for key in ["buffers", "images"] {
                    if let Some(items) = doc.get(key).and_then(Value::as_array) {
                        for item in items {
                            if let Some(uri) = item.get("uri").and_then(Value::as_str) {
                                self.dependency_reference(source, uri)?;
                            }
                        }
                    }
                }
            }
            "obj" | "mtl" => {
                let text = std::str::from_utf8(bytes).map_err(|e| EngineError::Other(format!("{source}: {e}")))?;
                for line in text.lines() {
                    let mut words = line.split_whitespace();
                    let Some(keyword) = words.next() else {
                        continue;
                    };
                    if extension == "obj" && keyword == "mtllib" {
                        for value in words {
                            self.dependency_reference(source, value)?;
                        }
                    } else if extension == "mtl" && (keyword.to_ascii_lowercase().starts_with("map_") || matches!(keyword, "bump" | "disp" | "norm")) {
                        if let Some(value) = words.last() {
                            self.dependency_reference(source, value)?;
                        }
                    }
                }
            }
            "mtlx" => {
                let wrapped = wrapped_xml(bytes)?;
                let doc = filmcraft_interchange::scene::read::read(&wrapped, &xml_limits()).map_err(|e| EngineError::Other(e.to_string()))?;
                for node in doc.doc.descendants().filter(|n| n.is_element()) {
                    if node.tag_name().name() == "include" {
                        return Err(EngineError::Other(format!("{source}: MaterialX includes are unsupported")));
                    }
                    if node.attribute("type") == Some("filename")
                        && let Some(value) = node.attribute("value")
                    {
                        self.dependency_reference(source, value)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn dependency_reference(&mut self, source: &str, value: &str) -> Result<()> {
        if value.starts_with("data:") || value.is_empty() {
            return Ok(());
        }
        // These formats have different URI readers. Reject ambiguous spellings instead of
        // validating one spelling and letting a downstream loader open another.
        if local_reference(value, false)? != value {
            return Err(EngineError::Other(format!("{source}: escaped or suffixed dependency paths are unsupported: {value}")));
        }
        self.reference(source, value, false, false)?;
        Ok(())
    }
}

fn normalize_graph(s: &Session, root: &str, primary: &str, package: &filmcraft_vpkg::Package) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut graph = ReferenceGraph {
        session: s,
        root,
        files: package.entries.iter().map(|e| (e.path.as_str(), e.bytes.as_slice())).collect(),
        seen: BTreeSet::new(),
        pending: Vec::new(),
        work: 0,
        generated_bytes: 0,
    };
    graph.enqueue(primary, true)?;
    let mut documents = BTreeMap::new();
    while let Some((source, document)) = graph.pending.pop() {
        let bytes = *graph.files.get(source.as_str()).ok_or_else(|| EngineError::Other(format!("{source}: dependency is not packaged")))?;
        if document {
            let normalized = graph.normalize(&source, bytes)?;
            graph.generated_bytes = graph
                .generated_bytes
                .checked_add(normalized.len())
                .filter(|n| *n <= Limits::default().max_total_bytes)
                .ok_or_else(|| EngineError::Other("normalized documents exceed byte budget".into()))?;
            documents.insert(source, normalized);
        } else {
            graph.dependency(&source, bytes)?;
        }
    }
    Ok(documents)
}

fn resolve_reference(source: &str, value: &str, sequence: bool) -> Result<String> {
    let value = value.trim();
    if value.contains(':') {
        return Err(EngineError::Other(format!("{source}: unverified nonlocal package reference: {value}")));
    }
    let decoded = local_reference(value, sequence)?;
    if decoded.is_empty() || decoded.starts_with('/') || decoded.contains(['\\', ':']) || decoded.chars().any(char::is_control) {
        return Err(EngineError::Other(format!("{source}: unsafe package reference {value}")));
    }
    let base = source.rsplit_once('/').map_or("", |(base, _)| base);
    let mut parts: Vec<_> = base.split('/').collect();
    for part in decoded.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.len() <= 1 {
                    return Err(EngineError::Other(format!("{source}: reference escapes project root")));
                }
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    let relative = parts.join("/");
    if relative.len() > Limits::default().max_name_bytes || !filmcraft_vpkg::zip::safe_path(&relative) {
        return Err(EngineError::Other(format!("{source}: unsafe or oversized package reference")));
    }
    Ok(relative)
}

fn rewrite_references(bytes: &[u8], mut rewrite: impl FnMut(&str, &str, &str, bool) -> Result<String>) -> Result<Vec<u8>> {
    let limits = xml_limits();
    let wrapped = wrapped_xml(bytes)?;
    let text = wrapped.as_str();
    let doc = filmcraft_interchange::scene::read::read(text, &limits).map_err(|e| EngineError::Other(e.to_string()))?;
    let mut out = String::new();
    let mut cursor = 0;
    let mut push = |part: &str| -> Result<()> {
        if out.len().saturating_add(part.len()) > limits.max_bytes {
            return Err(EngineError::Other("normalized scene exceeds byte budget".into()));
        }
        out.try_reserve(part.len()).map_err(|e| EngineError::Other(e.to_string()))?;
        out.push_str(part);
        Ok(())
    };
    for node in doc.doc.descendants() {
        for attr in node.attributes() {
            let tag = node.tag_name().name();
            let name = attr.name();
            if tag == "include" && name == "sha256" {
                // The archive verified the original; the derived include has normalized bytes.
                let range = attr.range();
                push(text.get(cursor..range.start).ok_or_else(|| EngineError::Other("invalid include hash range".into()))?)?;
                cursor = range.end;
                continue;
            }
            if !input_attribute(tag, name) || nonfile_reference(tag, name, attr.value()) {
                continue;
            }
            let sequence = matches!(tag, "imageSequence" | "meshSequence") || (tag == "volume" && node.attribute("first").is_some());
            if tag == "program"
                || tag == "lottie"
                || (tag == "volume" && node.attribute("format") == Some("srvseq"))
                || (tag == "basemap" && name == "mapStyle")
                || (tag == "colorManagement" && name == "ocioConfig")
            {
                return Err(EngineError::Other(format!("{tag}@{name}: package dependency format cannot be safely inspected")));
            }
            let value = rewrite(tag, name, attr.value(), sequence)?;
            let range = attr.range_value();
            push(text.get(cursor..range.start).ok_or_else(|| EngineError::Other("invalid scene attribute range".into()))?)?;
            for ch in value.chars() {
                match ch {
                    '&' => push("&amp;")?,
                    '<' => push("&lt;")?,
                    '"' => push("&quot;")?,
                    '\'' => push("&apos;")?,
                    _ => push(ch.encode_utf8(&mut [0; 4]))?,
                }
            }
            cursor = range.end;
        }
    }
    push(text.get(cursor..).ok_or_else(|| EngineError::Other("invalid scene tail range".into()))?)?;
    let body = out.strip_prefix(XML_OPEN).and_then(|s| s.strip_suffix(XML_CLOSE)).ok_or_else(|| EngineError::Other("invalid normalized XML wrapper".into()))?;
    Ok(body.as_bytes().to_vec())
}

const XML_OPEN: &str = "<scene version=\"1.7\">";
const XML_CLOSE: &str = "</scene>";
fn xml_limits() -> filmcraft_interchange::scene::read::Limits {
    filmcraft_interchange::scene::read::Limits {
        max_bytes: Limits::default().max_metadata_bytes,
        max_nodes: 100_000,
        max_depth: 66,
        max_attr_len: 4096,
        max_text_len: Limits::default().max_metadata_bytes,
        max_ids: 100_000,
    }
}
fn wrapped_xml(bytes: &[u8]) -> Result<String> {
    let limits = xml_limits();
    let text = filmcraft_interchange::scene::read::decode(bytes, &limits).map_err(|e| EngineError::Other(e.to_string()))?.trim_start();
    let text = if text.starts_with("<?xml") {
        text.split_once("?>").map(|(_, body)| body).ok_or_else(|| EngineError::Other("unterminated XML declaration".into()))?
    } else {
        text
    };
    if text.len().saturating_add(XML_OPEN.len()).saturating_add(XML_CLOSE.len()) > limits.max_bytes {
        return Err(EngineError::Other("dependency XML exceeds byte budget".into()));
    }
    Ok(format!("{XML_OPEN}{text}{XML_CLOSE}"))
}
fn nonfile_reference(tag: &str, name: &str, value: &str) -> bool {
    let value = value.trim();
    value.is_empty()
        || value.starts_with("data:")
        || (tag == "basemap" && name == "mapStyle" && matches!(value, "protomaps-light" | "protomaps-dark"))
        || (tag == "material" && name.ends_with("Map") && value.starts_with('#'))
        || (tag == "colorManagement" && name == "ocioConfig" && value.starts_with("ocio://"))
        || (tag == "captionTrack" && name == "model" && !value.contains(['/', '\\']) && !value.to_ascii_lowercase().ends_with(".bin"))
}

fn slug(name: &str) -> String {
    let mut result = String::new();
    for part in name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|s| !s.is_empty()) {
        if !result.is_empty() {
            result.push('-');
        }
        result.push_str(&part.to_ascii_lowercase());
    }
    if result.is_empty() { "sequence".into() } else { result }
}
fn metadata(p: &Value, name: &str) -> Result<Metadata> {
    if name.len() > 16 << 10 {
        return Err(crate::commands::bad(EXPORT, "sequence name exceeds 16 KiB"));
    }
    let id = if p.get("id").is_some() { string(p, "id", EXPORT)?.to_string() } else { slug(name) };
    let version = if p.get("version").is_some() { string(p, "version", EXPORT)? } else { "1.0.0" };
    let title = if p.get("title").is_some() { string(p, "title", EXPORT)? } else { name };
    if !id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
    {
        return Err(crate::commands::bad(EXPORT, "`id` must match [a-z0-9][a-z0-9._-]*"));
    }
    let mut version_parts = version.splitn(2, ['-', '+']);
    let base = version_parts.next().unwrap_or("");
    let suffix = version_parts.next();
    let numbers: Vec<_> = base.split('.').collect();
    if numbers.len() != 3
        || numbers.iter().any(|n| n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()))
        || suffix.is_some_and(|s| s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
    {
        return Err(crate::commands::bad(EXPORT, "`version` must be major.minor.patch with an optional - or + suffix"));
    }
    if title.is_empty() || title.chars().any(char::is_control) {
        return Err(crate::commands::bad(EXPORT, "title must be nonempty without control characters"));
    }
    Ok(Metadata { id, version: version.into(), title: title.into(), created: None })
}
fn asset_name(source: &str, index: usize) -> String {
    let file = Path::new(source.rsplit(['/', '\\']).next().unwrap_or("media"));
    let sanitize =
        |value: &str, max| value.chars().take(max).map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' }).collect::<String>();
    let stem = sanitize(file.file_stem().and_then(|s| s.to_str()).unwrap_or("media"), 120);
    let extension = file.extension().and_then(|s| s.to_str()).map(|s| sanitize(s, 16)).filter(|s| !s.is_empty());
    format!("assets/{index:04}-{stem}{}", extension.map(|s| format!(".{s}")).unwrap_or_default())
}

/// Export the selected sequence with its referenced media as a deterministic format 1.1 package.
pub fn export(s: &mut Session, p: &Value) -> Result<Value> {
    params(p, EXPORT, &["path", "sequence", "id", "version", "title"])?;
    let target = path(p, "path", EXPORT)?;
    let seq = match p.get("sequence") {
        None => s.state.active_sequence.ok_or(EngineError::NoSequence)?,
        Some(value) => ItemId(value.as_u64().ok_or_else(|| crate::commands::bad(EXPORT, "`sequence` must be an unsigned item ID"))?),
    };
    let name = &s.project.item(seq).ok_or(EngineError::NoSequence)?.name;
    let metadata = metadata(p, name)?;
    let opts = ExportOptions { name: Some(name.clone()), ..Default::default() };
    let (document, report) = crate::interchange::export_document(s, seq, Format::Scene, &opts)?;
    let limits = Limits::default();
    let text = std::str::from_utf8(&document).map_err(|e| EngineError::Other(e.to_string()))?;
    let doc = filmcraft_interchange::scene::read::read(
        text,
        &filmcraft_interchange::scene::read::Limits { max_bytes: limits.max_metadata_bytes, ..Default::default() },
    )
    .map_err(|e| EngineError::Other(e.to_string()))?;
    let mut sources = BTreeSet::new();
    for node in doc.doc.descendants().filter(|n| n.is_element()) {
        if let Some(source) = node.attribute("src") {
            if !matches!(node.tag_name().name(), "image" | "video" | "audio") {
                return Err(EngineError::Other(format!("unsupported exported input on {}", node.tag_name().name())));
            }
            sources.insert(source);
            if sources.len().saturating_add(4) > limits.max_entries {
                return Err(crate::commands::bad(EXPORT, "too many package media files"));
            }
        }
    }
    let mut names = BTreeMap::new();
    let mut files = Vec::new();
    let mut total = document.len();
    for (index, source) in sources.into_iter().enumerate() {
        let remaining = limits.max_total_bytes.checked_sub(total).ok_or_else(|| crate::commands::bad(EXPORT, "package exceeds expanded byte budget"))?;
        let bytes = read(s, source, limits.max_entry_bytes.min(remaining), EXPORT)?;
        total = total.checked_add(bytes.len()).ok_or_else(|| crate::commands::bad(EXPORT, "package size overflow"))?;
        let name = asset_name(source, index);
        names.insert(source, name.clone());
        files.push(FileInput { path: name, bytes, role: "asset".into() });
    }
    // Attribute byte ranges come from the bounded parser; splice with checked UTF-8 slices.
    let mut rewritten = String::new();
    rewritten
        .try_reserve(limits.max_metadata_bytes.min(document.len().saturating_add(names.len().saturating_mul(160))))
        .map_err(|e| EngineError::Other(e.to_string()))?;
    let mut cursor = 0;
    for node in doc.doc.descendants() {
        if let Some(attribute) = node.attribute_node("src") {
            let range = attribute.range_value();
            let name = names.get(attribute.value()).ok_or_else(|| EngineError::Other("missing media rewrite".into()))?;
            let prefix = text.get(cursor..range.start).ok_or_else(|| EngineError::Other("invalid XML attribute range".into()))?;
            if rewritten.len().saturating_add(prefix.len()).saturating_add(name.len()) > limits.max_metadata_bytes {
                return Err(crate::commands::bad(EXPORT, "rewritten scene exceeds byte budget"));
            }
            rewritten.push_str(prefix);
            rewritten.push_str(name);
            cursor = range.end;
        }
    }
    let tail = text.get(cursor..).ok_or_else(|| EngineError::Other("invalid XML tail range".into()))?;
    if rewritten.len().saturating_add(tail.len()) > limits.max_metadata_bytes {
        return Err(crate::commands::bad(EXPORT, "rewritten scene exceeds byte budget"));
    }
    rewritten.push_str(tail);
    let archive = filmcraft_vpkg::pack(&PackInput {
        primary: FileInput { path: "main.scene".into(), bytes: rewritten.into_bytes(), role: "scene".into() },
        files,
        metadata,
        limits,
        source_date_epoch: None,
    })
    .map_err(|e| invalid(target, e))?;
    s.services.write_file(target, &archive).map_err(|e| io(target, e))?;
    Ok(json!({"path":target,"bytes":archive.len(),"report":crate::interchange::report_json(&report)}))
}

// Windows canonical paths use verbatim prefixes; URI decoding returns ordinary drive/UNC paths.
pub(crate) fn relative_path(root: &str, target: &str) -> Option<String> {
    fn normalized(path: &str) -> (String, bool) {
        let slashes = path.replace('\\', "/");
        let ordinary =
            if let Some(unc) = slashes.strip_prefix("//?/UNC/") { format!("//{unc}") } else { slashes.strip_prefix("//?/").unwrap_or(&slashes).to_string() };
        let windows = ordinary.starts_with("//") || ordinary.as_bytes().get(1) == Some(&b':');
        (ordinary, windows)
    }
    let (root, windows) = normalized(root);
    let (target, target_windows) = normalized(target);
    if windows != target_windows {
        return None;
    }
    let mut target_parts = target.split('/');
    for part in root.trim_end_matches('/').split('/') {
        let candidate = target_parts.next()?;
        if if windows { !part.eq_ignore_ascii_case(candidate) } else { part != candidate } {
            return None;
        }
    }
    let relative = target_parts.collect::<Vec<_>>().join("/");
    if filmcraft_vpkg::zip::safe_path(&relative) { Some(relative) } else { None }
}
