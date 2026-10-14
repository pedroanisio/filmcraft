//! Scene document I/O and renderer preferences shared by import and the media pool.

use std::path::Path;

use filmcraft_media::{MediaError, Result};
use filmcraft_scenerender::{Config, Env, Opened, Quality};
use serde_json::{Value, json};

use crate::{EngineError, Services, Session};

pub(crate) fn is_scene(path: &str) -> bool {
    Path::new(path).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("scene"))
}

fn io(error: std::io::Error) -> MediaError {
    if error.kind() == std::io::ErrorKind::NotFound { MediaError::Offline(error.to_string()) } else { MediaError::Io(error.to_string()) }
}

pub(crate) fn read(path: &str, services: &dyn Services) -> Result<Vec<u8>> {
    let limit = filmcraft_interchange::scene::read::Limits::default().max_bytes;
    let too_large = || MediaError::Unsupported(format!("scene document exceeds {limit} bytes"));
    if let Some(reader) = services.reader(path) {
        let reader = reader.map_err(io)?;
        let len = usize::try_from(reader.len()).ok().filter(|len| *len <= limit).ok_or_else(too_large)?;
        return filmcraft_media::reader::read_range(&*reader, 0, len).map_err(io);
    }
    if services.file_size(path).map_err(io)? > limit as u64 {
        return Err(too_large());
    }
    let bytes = services.read_range(path, 0, limit.saturating_add(1)).map_err(io)?;
    if bytes.len() > limit {
        return Err(too_large());
    }
    Ok(bytes)
}

pub(crate) fn open(path: &str, bytes: &[u8], env: &Env) -> Result<Opened> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if path.len() > 16 << 10 {
            return Err(MediaError::Unsupported("scene document path exceeds 16 KiB".into()));
        }
        let path = std::path::absolute(path).map_err(io)?;
        filmcraft_scenerender::open(&path, bytes, env)
    }
    // The web cannot spawn the renderer: the scene opens as its placeholder slate. Browser paths may
    // lack a root, which the bridge requires.
    #[cfg(target_arch = "wasm32")]
    {
        let rooted = Path::new(path);
        let rooted = if rooted.is_absolute() { rooted.to_path_buf() } else { Path::new("/").join(rooted) };
        filmcraft_scenerender::open(&rooted, bytes, env)
    }
}

pub(crate) fn virtual_placeholder(path: &str, bytes: &[u8]) -> Result<Opened> {
    if path.len() > 16 << 10 {
        return Err(MediaError::Unsupported("scene document path exceeds 16 KiB".into()));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let rooted = std::path::absolute(path).map_err(io)?;
    #[cfg(target_arch = "wasm32")]
    let rooted = if Path::new(path).is_absolute() { Path::new(path).to_path_buf() } else { Path::new("/").join(path) };
    // No executable is supplied, so discovery and process creation are bypassed entirely.
    let mut opened =
        filmcraft_scenerender::open_with(&rooted, bytes, &Env::default(), None, std::sync::Arc::new(filmcraft_scenerender::process::ProcessRunner))?;
    let reason = "scene-render requires filesystem-backed paths; this host stores the scene in virtual file services".to_string();
    opened.status.reason = Some(reason.clone());
    opened.warning = Some(reason);
    Ok(opened)
}

impl Session {
    pub(crate) fn apply_scene_prefs(&mut self) {
        let path = self.prefs.plugins.scene_render_path.trim();
        let config = Config { program: (!path.is_empty()).then(|| path.into()) };
        let quality = if self.prefs.plugins.scene_render_preview_quality == "draft" { Quality::Draft } else { Quality::Preview };
        if self.media.set_scene_env(config, self.media_cache_dir(), quality) {
            self.bump_view();
        }
    }
}

fn empty_params(params: &Value, command: &str) -> crate::Result<()> {
    if !params.as_object().is_some_and(|object| object.is_empty()) {
        return Err(crate::commands::bad(command, "expected an empty object"));
    }
    Ok(())
}

pub(crate) fn status(s: &Session, params: &Value) -> crate::Result<Value> {
    empty_params(params, "scene.renderer.status")?;
    let config = Config { program: (!s.prefs.plugins.scene_render_path.trim().is_empty()).then(|| s.prefs.plugins.scene_render_path.trim().into()) };
    serde_json::to_value(filmcraft_scenerender::probe(&config)).map_err(|error| EngineError::Other(error.to_string()))
}

pub(crate) fn clear_cache(s: &mut Session, params: &Value) -> crate::Result<Value> {
    empty_params(params, "scene.renderer.clearCache")?;
    let dir = s.media_cache_dir().ok_or_else(|| EngineError::Other("there is no media cache folder in this session".into()))?;
    let bytes = filmcraft_scenerender::store::Store::new(&dir)?.clear()?;
    s.media.clear_scenes();
    s.bump_view();
    Ok(json!({"bytes": bytes}))
}
