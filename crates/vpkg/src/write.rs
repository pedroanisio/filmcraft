//! Deterministic, in-memory format 1.1 package construction.
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

use crate::{Error, Limits, manifest::Metadata};
use crate::{invalid, manifest, verify, zip};
use filmcraft_interchange::scene::values;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// One project file supplied by the caller.
#[derive(Debug, Clone)]
pub struct FileInput {
    pub path: String,
    pub bytes: Vec<u8>,
    pub role: String,
}
/// Everything required to pack a project without filesystem access.
#[derive(Debug)]
pub struct PackInput {
    pub primary: FileInput,
    pub files: Vec<FileInput>,
    pub metadata: Metadata,
    pub limits: Limits,
    pub source_date_epoch: Option<u64>,
}

/// Build a deterministic format 1.1 archive from explicitly supplied project files.
/// The caller supplies every input; fonts, downloads and pipelines are never resolved or run.
pub fn pack(input: &PackInput) -> Result<Vec<u8>, Error> {
    let limits = &input.limits;
    if input.files.len().saturating_add(4) > limits.max_entries {
        return Err(invalid("LIMIT", "files", "too many entries including metadata"));
    }
    if input.primary.role != "scene" {
        return Err(invalid("PRIMARY", &input.primary.path, "primary file must have role scene"));
    }
    zip::unique_paths(std::iter::once(input.primary.path.as_str()).chain(input.files.iter().map(|f| f.path.as_str())), limits)?;
    let mut files: BTreeMap<&str, &FileInput> = BTreeMap::new();
    let mut total = 0usize;
    let mut names = 0usize;
    for f in std::iter::once(&input.primary).chain(&input.files) {
        if f.bytes.len() > limits.max_entry_bytes || f.role.len() > 64 {
            return Err(invalid("LIMIT", &f.path, "file size or role exceeds budget"));
        }
        total = total.checked_add(f.bytes.len()).ok_or_else(|| invalid("LIMIT", "files", "total file size overflow"))?;
        names = names.saturating_add(f.path.len());
        files.insert(&f.path, f);
    }
    let metadata_len = input
        .metadata
        .id
        .len()
        .saturating_add(input.metadata.title.len())
        .saturating_add(input.metadata.version.len())
        .saturating_add(input.metadata.created.as_ref().map_or(0, String::len));
    if total > limits.max_total_bytes || metadata_len > limits.max_metadata_bytes {
        return Err(invalid("LIMIT", "files", "input budget exceeded"));
    }
    let reserve = names.saturating_add(metadata_len).saturating_mul(256).saturating_add(files.len().saturating_mul(64 << 10)).saturating_add(4 << 20);
    let mut remaining = *limits;
    remaining.max_memory_bytes = limits
        .max_memory_bytes
        .checked_sub(total.saturating_add(reserve))
        .ok_or_else(|| invalid("LIMIT", "files", "packing working-memory budget exceeded"))?;
    let doc = verify::scene_doc(&input.primary.bytes, &remaining)?;
    let project = doc
        .doc
        .descendants()
        .find(|n| n.has_tag_name("project"))
        .ok_or_else(|| invalid("VIDEO", &input.primary.path, "primary scene has no project element"))?;
    let dimension = |name: &str| -> Result<u32, Error> {
        project
            .attribute(name)
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&v| v > 0)
            .ok_or_else(|| invalid("VIDEO", name, "expected a positive integer dimension"))
    };
    let width = dimension("width")?;
    let height = dimension("height")?;
    let fps = project.attribute("fps").ok_or_else(|| invalid("VIDEO", "fps", "missing frame rate"))?;
    values::fps(fps).map_err(|e| invalid("VIDEO", "fps", &e))?;
    let duration = project
        .attribute("duration")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| invalid("VIDEO", "duration", "expected a finite positive duration"))?;
    let audio = doc.doc.descendants().any(|n| n.has_tag_name("audio") && n.attribute("src").is_some_and(|s| !s.is_empty()));
    let captions: BTreeSet<&str> = doc
        .doc
        .descendants()
        .filter(|n| n.has_tag_name("captionTrack"))
        .filter_map(|n| {
            n.attribute("language").filter(|s| !s.is_empty()).or_else(|| n.attribute("lang").filter(|s| !s.is_empty())).or_else(|| n.attribute("id"))
        })
        .filter(|s| !s.is_empty())
        .collect();
    let outputs: Vec<Value> = doc
        .doc
        .descendants()
        .filter(|n| n.has_tag_name("output"))
        .filter_map(|n| n.attribute("id").map(|id| json!({"id":id,"path":n.attribute("path").unwrap_or("")})))
        .collect();
    let listed: Vec<Value> = files
        .iter()
        .map(|(path, f)| {
            let mut entry = json!({"path":path,"size":f.bytes.len(),"sha256":verify::hash(&f.bytes),"role":f.role});
            if let Some(kind) = media_type(path)
                && let Some(object) = entry.as_object_mut()
            {
                object.insert("mediaType".into(), json!(kind));
            }
            entry
        })
        .collect();
    let mut raw = json!({"format":"scene-video-package","formatVersion":"1.1","package":input.metadata,"scenes":[{"path":input.primary.path,"role":"primary"}],"video":{"scene":input.primary.path,"schemaVersion":doc.version.to_string(),"width":width,"height":height,"fps":fps,"duration":duration,"audio":audio,"captions":captions,"outputs":outputs},"files":listed,"packed":{"tool":"filmcraft","toolVersion":env!("CARGO_PKG_VERSION"),"fileCount":files.len(),"totalSize":total}});
    let initial = json_bytes(&raw, limits)?;
    let manifest = manifest::read(&initial, &remaining)?;
    let project: BTreeMap<&str, &[u8]> = files.iter().map(|(path, f)| (*path, f.bytes.as_slice())).collect();
    let facts = verify::check_references(&manifest, &project, &remaining, true)?;
    if let Some(video) = raw.get_mut("video").and_then(Value::as_object_mut) {
        video.insert("audio".into(), json!(facts.audio));
    }
    let readme = readme(&input.metadata, &input.primary.path, width, height, fps, duration, files.len(), total);
    if let Some(packed) = raw.get_mut("packed").and_then(Value::as_object_mut) {
        packed.insert("metaSha256".into(), json!({"README.md":verify::hash(readme.as_bytes()),"vpkg.schema.json":verify::hash(manifest::SCHEMA)}));
    } else {
        return Err(invalid("MANIFEST", "packed", "missing generated packing metadata"));
    }
    let manifest_bytes = json_bytes(&raw, limits)?;
    let names: Vec<String> = files.keys().map(|path| format!("project/{path}")).collect();
    let mut entries = vec![
        zip::Input { path: "vpkg.json", bytes: &manifest_bytes, executable: false },
        zip::Input { path: "vpkg.schema.json", bytes: manifest::SCHEMA, executable: false },
        zip::Input { path: "README.md", bytes: readme.as_bytes(), executable: false },
    ];
    for (name, file) in names.iter().zip(files.values()) {
        entries.push(zip::Input { path: name, bytes: &file.bytes, executable: file.role == "script" });
    }
    zip::write(&entries, &remaining, input.source_date_epoch)
}
fn json_bytes(value: &Value, limits: &Limits) -> Result<Vec<u8>, Error> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| invalid("MANIFEST", "vpkg.json", &e.to_string()))?;
    bytes.push(b'\n');
    if bytes.len() > limits.max_metadata_bytes {
        return Err(invalid("LIMIT", "vpkg.json", "generated manifest exceeds metadata budget"));
    }
    Ok(bytes)
}
fn readme(metadata: &Metadata, scene: &str, width: u32, height: u32, fps: &str, duration: f64, count: usize, total: usize) -> String {
    format!(
        "# {}\n\npackage `{}` {} · {}×{} · {} fps · {} s\n\nThis is a scene video package (format `scene-video-package` 1.1).\n`vpkg.json` records each file's size and SHA-256.\n\n## Use\n\n```bash\nscenerender-vpkg verify {}-{}.vpkg.zip\nscenerender-vpkg unpack {}-{}.vpkg.zip {}\n```\n\n## Scenes\n\n- `{scene}` — primary\n\n## Contents\n\n{count} project files, {total} bytes.\n\nFilmCraft packages explicitly supplied files. It does not resolve host fonts, lint scripts, download inputs or run build pipelines.\n\n## Schema license\n\n`vpkg.schema.json` from sr-core is distributed under the following MIT license:\n\n{schema_license}",
        metadata.title,
        metadata.id,
        metadata.version,
        width,
        height,
        fps,
        duration,
        metadata.id,
        metadata.version,
        metadata.id,
        metadata.version,
        metadata.id,
        schema_license = include_str!("../../interchange/schema/sr-core/LICENSE")
    )
}
fn media_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "scene" | "xml" | "xsd" | "sch" => "application/xml",
        "json" | "topojson" => "application/json",
        "geojson" => "application/geo+json",
        "kml" => "application/vnd.google-earth.kml+xml",
        "gpx" => "application/gpx+xml",
        "md" => "text/markdown",
        "txt" | "glsl" | "conf" => "text/plain",
        "csv" => "text/csv",
        "srt" => "application/x-subrip",
        "vtt" => "text/vtt",
        "ass" => "text/x-ssa",
        "html" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "py" => "text/x-python",
        "sh" => "application/x-sh",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "exr" => "image/x-exr",
        "hdr" => "image/vnd.radiance",
        "ktx2" => "image/ktx2",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "gltf" => "model/gltf+json",
        "glb" => "model/gltf-binary",
        "obj" => "model/obj",
        "mtl" => "model/mtl",
        "stl" => "model/stl",
        "ply" => "model/ply",
        "usdz" => "model/vnd.usdz+zip",
        "fbx" | "bin" | "onnx" => "application/octet-stream",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "ttc" => "font/collection",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => return None,
    })
}
