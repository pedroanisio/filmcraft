//! Interchange documents (CMX 3600 EDL, FCP7 XML, FCPXML, OTIO, AAF, OMF, sr-core scene) ↔ the session's project.

use serde_json::{Value, json};

use filmcraft_interchange::{ExportOptions, Format, ImportOptions};
use filmcraft_project::{ItemId, ItemKind, MediaRef};

use crate::{EngineError, Result, Session};

/// The interchange format of a file, if it is one (by content, with the extension as a hint).
pub fn detect(path: &str, bytes: &[u8]) -> Option<Format> {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if !matches!(ext.as_deref(), Some("edl" | "xml" | "fcpxml" | "otio" | "aaf" | "omf" | "omfi" | "scene")) {
        return None;
    }
    filmcraft_interchange::detect(bytes, ext.as_deref())
}

/// Report messages shared by document and package command results.
pub(crate) fn report_json(r: &filmcraft_interchange::Report) -> Vec<String> {
    r.entries.iter().map(|e| if e.count > 1 { format!("{} (×{})", e.message, e.count) } else { e.message.clone() }).collect()
}

fn is_file_media(item: &filmcraft_project::ProjectItem) -> bool {
    matches!(&item.kind, ItemKind::Media(m) if matches!(m.media, MediaRef::File { .. }))
}

/// Import a document: merge its bins, media and sequences into the project (one undo step), then
/// link each media file that exists on disk. Returns the new sequences.
pub fn import(s: &mut Session, path: &str, bytes: &[u8], format: Format) -> Result<Value> {
    import_lenient(s, path, bytes, format, false)
}

/// Import a document with explicit control over recoverable scene validation errors and losses.
pub fn import_lenient(s: &mut Session, path: &str, bytes: &[u8], format: Format, lenient: bool) -> Result<Value> {
    let opts = options(path, lenient, format);
    let (fragment, report) = read_fragment(s, path, bytes, format, &opts)?;
    merge(s, fragment, &report, format, opts.name.as_deref().unwrap_or(format.name()))
}

fn options(path: &str, lenient: bool, format: Format) -> ImportOptions {
    let p = std::path::Path::new(path);
    ImportOptions {
        lenient,
        base_dir: p.parent().map(|d| d.to_string_lossy().to_string()),
        name: (if format == Format::Scene { p.file_name() } else { p.file_stem() }).map(|n| n.to_string_lossy().to_string()),
        ..Default::default()
    }
}

fn import_error(path: &str, error: filmcraft_interchange::Error) -> EngineError {
    let message = format!("{path}: {error}");
    match error {
        filmcraft_interchange::Error::Invalid(diagnostics) => EngineError::InvalidScene { message, diagnostics },
        _ => EngineError::Other(message),
    }
}

fn read_fragment(
    s: &Session,
    path: &str,
    bytes: &[u8],
    format: Format,
    opts: &ImportOptions,
) -> Result<(filmcraft_interchange::Imported, filmcraft_interchange::Report)> {
    match format {
        // AAF / OMF may embed audio: write it next to the document (the media items point there)
        Format::Aaf | Format::Omf => {
            let r = if format == Format::Aaf { filmcraft_interchange::aaf::import(bytes, opts) } else { filmcraft_interchange::omf::import(bytes, opts) };
            let (fragment, extracted, report) = r.map_err(|e| import_error(path, e))?;
            for m in extracted {
                if let Some(dir) = std::path::Path::new(&m.path).parent().filter(|_| !s.services.export_in_memory()) {
                    let _ = std::fs::create_dir_all(dir);
                }
                s.services.write_file(&m.path, &m.wav).map_err(|e| EngineError::Other(format!("{}: {e}", m.path)))?;
            }
            Ok((fragment, report))
        }
        _ => filmcraft_interchange::import_with(bytes, format, opts).map_err(|e| import_error(path, e)),
    }
}

fn merge(s: &mut Session, fragment: filmcraft_interchange::Imported, report: &filmcraft_interchange::Report, format: Format, name: &str) -> Result<Value> {
    if format != Format::Scene {
        return merge_and_link(s, fragment, report, format, name);
    }
    let before: std::collections::HashSet<ItemId> = s.project.items.keys().copied().collect();
    let media = s.media.clone();
    let services = s.services.clone();
    let mut links = Vec::new();
    let mut offline = Vec::new();
    // edit_group retains every intermediate project snapshot. Link inside this single edit so
    // even a scene with thousands of distinct assets needs only the pre-import undo snapshot.
    let seqs = s.edit(&format!("Import {name}"), |proj, _| {
        let seqs = filmcraft_interchange::merge_into(proj, fragment, None);
        let mut linked_ids = std::collections::HashSet::new();
        for item in proj.items.values_mut().filter(|item| !before.contains(&item.id)) {
            let ItemKind::Media(m) = &mut item.kind else { continue };
            let MediaRef::File { path } = &m.media else { continue };
            match open_import_media(&media, &*services, path) {
                Ok((source, warning)) => {
                    m.info = source.info().clone();
                    m.offline = false;
                    m.identity = crate::relink::identity_of(&*services, path).ok();
                    linked_ids.insert(item.id);
                    links.push((item.id, path.clone(), source, warning));
                }
                Err(_) => offline.push(path.clone()),
            }
        }
        proj.resolve_placed_auto_points(|seq| !before.contains(&seq), |source| !is_file_media(source) || linked_ids.contains(&source.id));
        Ok(seqs)
    })?;
    let linked = links.len();
    let mut warnings = Vec::new();
    for (id, path, source, warning) in links {
        s.media.insert_file(id, &path, source);
        s.media.scene_warning(id, &path, warning.as_deref());
        if let Some(warning) = warning {
            warnings.push(format!("{path}: {warning}"));
        }
    }
    finish_import(s, &seqs, report, format, linked, offline, warnings)
}

fn open_import_media(
    media: &crate::media_pool::MediaPool,
    services: &dyn crate::Services,
    path: &str,
) -> filmcraft_media::Result<(filmcraft_media::SharedSource, Option<String>)> {
    if crate::scene_render::is_scene(path) {
        crate::scene_render::read(path, services)
            .and_then(|bytes| {
                if !services.is_filesystem_backed(path) { crate::scene_render::virtual_placeholder(path, &bytes) } else { media.open_scene_bytes(path, &bytes) }
            })
            .map(|opened| (opened.source, opened.warning))
    } else {
        media.open_file(path, services).map(|source| (source, None))
    }
}

fn merge_and_link(
    s: &mut Session,
    fragment: filmcraft_interchange::Imported,
    report: &filmcraft_interchange::Report,
    format: Format,
    name: &str,
) -> Result<Value> {
    let before: std::collections::HashSet<ItemId> = s.project.items.keys().copied().collect();
    let seqs = s.edit(&format!("Import {name}"), |proj, _| {
        let seqs = filmcraft_interchange::merge_into(proj, fragment, None);
        // A document that sets only some Motion parameters leaves the others at their "auto"
        // (NaN) defaults. Resolve them as placing a clip does, for every format. A media file's
        // picture size is only what the document says so far (an EDL says nothing: the importer
        // assumes the sequence size), so its clips' anchors wait for Link Media below.
        proj.resolve_placed_auto_points(|seq| !before.contains(&seq), |source| !is_file_media(source));
        Ok(seqs)
    })?;
    // Link media: probe each new file-backed item that exists, through the host's reader as a
    // direct import does (only the container index is read now, not the whole file: #461).
    let new_media: Vec<(ItemId, String)> = s
        .project
        .items
        .values()
        .filter(|i| !before.contains(&i.id))
        .filter_map(|i| match &i.kind {
            ItemKind::Media(m) => match &m.media {
                MediaRef::File { path } => Some((i.id, path.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let mut linked = 0;
    let mut offline = Vec::new();
    let mut warnings = Vec::new();
    for (id, mpath) in new_media {
        let opened = open_import_media(&s.media, &*s.services, &mpath);
        match opened {
            Ok((src, warning)) => {
                let info = src.info().clone();
                let identity = crate::relink::identity_of(&*s.services, &mpath).ok();
                let rebase = (format == Format::Edl).then_some(info.start_timecode).flatten();
                s.edit("Link Media", |proj, _| {
                    if let Some(item) = proj.items.get_mut(&id)
                        && let ItemKind::Media(m) = &mut item.kind
                    {
                        let rate = info.video.as_ref().map(|v| v.frame_rate);
                        m.info = info.clone();
                        m.offline = false;
                        m.identity = identity;
                        if let (Some(tc), Some(rate)) = (rebase, rate) {
                            filmcraft_interchange::rebase_source_timecode(proj, id, tc, rate);
                        }
                    }
                    // the picture size is now the file's own: centre the anchors of its clips in it
                    proj.resolve_placed_auto_points(|seq| !before.contains(&seq), |source| source.id == id);
                    Ok(())
                })?;
                s.media.insert_file(id, &mpath, src);
                s.media.scene_warning(id, &mpath, warning.as_deref());
                if let Some(warning) = warning {
                    warnings.push(format!("{mpath}: {warning}"));
                }
                linked += 1;
            }
            Err(_) => offline.push(mpath),
        }
    }
    finish_import(s, &seqs, report, format, linked, offline, warnings)
}

fn finish_import(
    s: &mut Session,
    seqs: &[ItemId],
    report: &filmcraft_interchange::Report,
    format: Format,
    linked: usize,
    offline: Vec<String>,
    warnings: Vec<String>,
) -> Result<Value> {
    if let Some(&first) = seqs.first() {
        s.state.active_sequence = Some(first);
        if !s.state.open_sequences.contains(&first) {
            s.state.open_sequences.push(first);
        }
    }
    Ok(json!({
        "format": format.name(),
        "sequences": seqs.iter().map(|i| i.0).collect::<Vec<_>>(),
        "linkedMedia": linked,
        "offlineMedia": offline,
        "report": report_json(report),
        "warnings": warnings,
    }))
}

pub(crate) fn lenient_param(params: &Value, command: &str) -> Result<bool> {
    match params.get("lenient") {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(crate::commands::bad(command, "`lenient` must be a boolean")),
    }
}

fn scene_path<'a>(params: &'a Value, command: &str) -> Result<&'a str> {
    let path = params.get("path").and_then(Value::as_str).ok_or_else(|| crate::commands::bad(command, "need string `path`"))?;
    if path.is_empty() || path.len() > 16 << 10 || path.contains('\0') || path.split(['/', '\\']).any(|part| part == "..") {
        return Err(crate::commands::bad(command, "`path` must be nonempty, at most 16 KiB, and contain neither NUL nor parent components"));
    }
    Ok(path)
}

/// Import a scene as rendered media, editable native content, or native content with rendered overlays.
pub fn import_scene(s: &mut Session, params: &Value) -> Result<Value> {
    const COMMAND: &str = "file.importScene";
    let object = params.as_object().ok_or_else(|| crate::commands::bad(COMMAND, "expected an object"))?;
    if object.keys().any(|key| !matches!(key.as_str(), "path" | "mode" | "lenient")) {
        return Err(crate::commands::bad(COMMAND, "unknown parameter (use path, mode, lenient)"));
    }
    let path = scene_path(params, COMMAND)?;
    let lenient = lenient_param(params, COMMAND)?;
    let mode = match params.get("mode") {
        None => "hybrid",
        Some(Value::String(mode)) if matches!(mode.as_str(), "rendered" | "native" | "hybrid") => mode,
        _ => return Err(crate::commands::bad(COMMAND, "`mode` must be rendered, native or hybrid")),
    };
    if mode == "rendered" {
        if !crate::scene_render::is_scene(path) {
            return Err(crate::commands::bad(COMMAND, "rendered mode requires a .scene path"));
        }
        return s.execute("file.import", json!({"paths": [path]}));
    }
    let bytes = crate::scene_render::read(path, &*s.services)?;
    let prepared = prepare_scene(s, path, &bytes, mode, lenient)?;
    // Write before editing: a full disk must not leave a partly merged project.
    for overlay in &prepared.overlays {
        write_overlay(s, &overlay.path, &overlay.document)?;
    }
    finish_scene(s, prepared)
}

/// A parsed scene ready for destination preflight, writes, and a single project merge.
pub(crate) struct PreparedScene {
    pub(crate) fragment: filmcraft_interchange::Imported,
    pub(crate) overlays: Vec<filmcraft_interchange::scene::import::Overlay>,
    report: filmcraft_interchange::Report,
    name: String,
}

/// Prepare without opening media or writing overlays, so package imports can preflight all paths.
pub(crate) fn prepare_scene(s: &Session, path: &str, bytes: &[u8], mode: &str, lenient: bool) -> Result<PreparedScene> {
    let opts = options(path, lenient, Format::Scene);
    let (fragment, overlays, report) = if mode == "native" {
        // The generic scene dispatcher already refuses overlays or drops them with a loss report.
        let (fragment, report) = read_fragment(s, path, bytes, Format::Scene, &opts)?;
        (fragment, Vec::new(), report)
    } else {
        filmcraft_interchange::scene::import(bytes, &opts).map_err(|e| import_error(path, e))?
    };
    Ok(PreparedScene { fragment, overlays, report, name: opts.name.unwrap_or_else(|| "Scene".into()) })
}

/// Merge a prepared scene after its caller has successfully written every overlay.
pub(crate) fn finish_scene(s: &mut Session, prepared: PreparedScene) -> Result<Value> {
    let PreparedScene { fragment, overlays, report, name } = prepared;
    let before: std::collections::HashSet<ItemId> = s.project.items.keys().copied().collect();
    let mut result = merge(s, fragment, &report, Format::Scene, &name)?;
    let paths: std::collections::HashSet<&str> = overlays.iter().map(|overlay| overlay.path.as_str()).collect();
    let merged: Vec<Value> = s
        .project
        .items
        .values()
        .filter(|item| !before.contains(&item.id))
        .filter_map(|item| {
            let ItemKind::Media(media) = &item.kind else { return None };
            let MediaRef::File { path } = &media.media else { return None };
            paths.contains(path.as_str()).then(|| json!({"item": item.id.0, "path": path}))
        })
        .collect();
    if let Some(object) = result.as_object_mut() {
        object.insert("overlays".into(), json!(merged));
    }
    Ok(result)
}

fn write_overlay(s: &Session, path: &str, bytes: &[u8]) -> Result<()> {
    // Re-import may reuse identical output, but must not replace another document's content.
    match s.services.file_size(path) {
        Ok(len) => {
            if len == bytes.len() as u64 && s.services.read_range(path, 0, bytes.len()).map_err(|e| EngineError::Other(format!("{path}: {e}")))? == bytes {
                return Ok(());
            }
            return Err(EngineError::Other(format!("{path}: overlay file already exists with different content")));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(EngineError::Other(format!("{path}: {error}"))),
    }
    s.services.create_parent_dirs(path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    s.services.write_file(path, bytes).map_err(|e| EngineError::Other(format!("{path}: {e}")))
}

/// `file.exportInterchange {format: "edl"|"xml"|"fcpxml"|"otio"|"aaf"|"omf"|"scene" = "xml", path, sequence?}`
pub fn export(s: &mut Session, p: &Value) -> Result<Value> {
    // FCP7 XML only when no format is asked for: a format we don't know is an error, not XML.
    // The names are the documented ones (each format's extension, in either case) and nothing
    // else, so the error can list exactly what is accepted.
    let format = match p.get("format") {
        None | Some(Value::Null) => Format::Fcp7Xml,
        Some(v) => v.as_str().and_then(|name| Format::ALL.into_iter().find(|f| f.extension().eq_ignore_ascii_case(name))).ok_or_else(|| {
            let names: Vec<&str> = Format::ALL.iter().map(|f| f.extension()).collect();
            crate::commands::bad("file.exportInterchange", format!("unknown format {v}: use one of {}", names.join(", ")))
        })?,
    };
    if format == Format::Scene {
        scene_path(p, "file.exportInterchange")?;
        if p.get("sequence").is_some_and(|value| value.as_u64().is_none()) {
            return Err(crate::commands::bad("file.exportInterchange", "`sequence` must be an unsigned item ID"));
        }
    }
    let path = p.get("path").and_then(Value::as_str).ok_or_else(|| EngineError::Other("need `path`".into()))?.to_string();
    let seq = p.get("sequence").and_then(Value::as_u64).map(ItemId).or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
    let opts = ExportOptions {
        relative_to: std::path::Path::new(&path).parent().map(|d| d.to_string_lossy().to_string()),
        name: s.project.item(seq).map(|i| i.name.clone()),
        ..Default::default()
    };
    let (bytes, report) = export_document(s, seq, format, &opts)?;
    s.services.write_file(&path, &bytes).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"path": path, "bytes": bytes.len(), "report": report_json(&report)}))
}

/// Shared serialization path for document files and scene packages.
pub(crate) fn export_document(s: &Session, seq: ItemId, format: Format, opts: &ExportOptions) -> Result<(Vec<u8>, filmcraft_interchange::Report)> {
    filmcraft_interchange::export(&s.project, seq, format, opts).map_err(|e| {
        let message = e.to_string();
        match e {
            filmcraft_interchange::Error::Invalid(diagnostics) => EngineError::InvalidScene { message, diagnostics },
            _ => EngineError::Other(message),
        }
    })
}

#[cfg(test)]
#[path = "scene_interchange_tests.rs"]
mod scene_tests;
