//! Bridge to **scene-render**, the external renderer for sr-core scene documents (`.scene`).
//!
//! A scene document imports as one rendered media item. Its frames (and audio) come from spawning the
//! separately installed `scene-render` program, which FilmCraft never links or ships. Rendered frames are
//! cached in memory and on disk. Without the renderer the item still opens, with its size, rate and duration
//! read from the document header, and shows a placeholder slate. See `docs/scene-render.md`.
//!
//! Layer L3. Spawning code is native only: on wasm the renderer is always unavailable.

pub mod args;
pub mod discover;
pub mod header;
pub mod ingest;
pub mod placeholder;
pub mod process;
pub mod source;
pub mod store;
pub mod timing;

#[cfg(all(test, unix))]
pub(crate) mod fake;

use serde::{Deserialize, Serialize};

/// Render quality passed to `scene-render render --quality`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Quality {
    Draft,
    #[default]
    Preview,
    Final,
}

impl Quality {
    /// The `--quality` argument value, also used as the disk-cache directory name.
    pub fn as_str(self) -> &'static str {
        match self {
            Quality::Draft => "draft",
            Quality::Preview => "preview",
            Quality::Final => "final",
        }
    }
}

use filmcraft_media::{MediaError, Result, SharedSource};

pub use discover::{Config, Status};
pub use source::PlaceholderSource;
#[cfg(not(target_arch = "wasm32"))]
pub use source::SceneRenderSource;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// Renderer settings and per-source cache limits.
#[derive(Clone, Debug)]
pub struct Env {
    /// Executable preference; otherwise search the environment and PATH.
    pub config: discover::Config,
    /// Media cache root; None retains video frames only in memory.
    pub cache_dir: Option<PathBuf>,
    /// Quality when the calling thread has no draft hint.
    pub quality: Quality,
    /// Frames per invocation (default 48, clamped to 1..=65536).
    pub chunk_len: u64,
    /// Per-invocation deadline (default ten minutes, capped at one hour).
    pub timeout: Duration,
    /// Decoded-frame budget (default and maximum 256 MiB).
    pub memory_budget: usize,
}
impl Default for Env {
    fn default() -> Self {
        Self {
            config: discover::Config::default(),
            cache_dir: None,
            quality: Quality::Preview,
            chunk_len: 48,
            timeout: Duration::from_secs(600),
            memory_budget: 256 << 20,
        }
    }
}
/// Opened media and renderer availability.
pub struct Opened {
    /// Video and audio provider, including the placeholder when unavailable.
    pub source: SharedSource,
    /// Renderer availability at open time.
    pub status: discover::Status,
    /// User-facing reason for placeholder media.
    pub warning: Option<String>,
}
/// Open an absolute scene path using cached system discovery; metadata comes only from bytes.
pub fn open(path: &Path, bytes: &[u8], env: &Env) -> Result<Opened> {
    let header = header::parse(bytes)?;
    let (status, program) = cached_probe(&env.config);
    finish_open(path, bytes, env, header, status, program, Arc::new(process::ProcessRunner))
}

/// Open with an injected runner and already located executable; None selects the placeholder.
/// Injected probes deliberately bypass the process-wide system-probe cache.
pub fn open_with(path: &Path, bytes: &[u8], env: &Env, program: Option<PathBuf>, runner: Arc<dyn process::Runner>) -> Result<Opened> {
    let header = header::parse(bytes)?;
    let status = probe_with(program.as_deref(), runner.as_ref(), env.timeout);
    finish_open(path, bytes, env, header, status, program, runner)
}

#[allow(clippy::too_many_arguments)]
fn finish_open(
    path: &Path,
    bytes: &[u8],
    env: &Env,
    header: header::SceneHeader,
    status: Status,
    program: Option<PathBuf>,
    runner: Arc<dyn process::Runner>,
) -> Result<Opened> {
    if !path.is_absolute() || path.as_os_str().len() > 16 << 10 {
        return Err(MediaError::Unsupported("scene document path must be absolute and at most 16 KiB".into()));
    }
    let info = source::media_info(path, bytes.len(), &header);
    #[cfg(not(target_arch = "wasm32"))]
    if status.found {
        let program = program.ok_or_else(|| MediaError::Unsupported("scene-render probe has no path".into()))?;
        let version = status.version.as_deref().ok_or_else(|| MediaError::Unsupported("scene-render probe has no version".into()))?;
        let source = source::SceneRenderSource::new(path, bytes, env, header, info, program, version, runner);
        return Ok(Opened { source: Arc::new(source), status, warning: None });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (env, program, runner);
    let source = Arc::new(source::PlaceholderSource::new(info, header));
    Ok(Opened { source, warning: status.reason.clone(), status })
}

fn unavailable(reason: impl Into<String>) -> Status {
    Status { reason: Some(reason.into()), ..Status::default() }
}

#[cfg(not(target_arch = "wasm32"))]
fn probe_with(program: Option<&Path>, runner: &dyn process::Runner, timeout: Duration) -> Status {
    let Some(program) = program else {
        return unavailable("scene-render not found: set Preferences › Plugins › scene-render, FILMCRAFT_SCENE_RENDER or PATH");
    };
    let mut status = Status { path: Some(program.to_string_lossy().into_owned()), ..Status::default() };
    let result = (|| -> Result<()> {
        if !program.is_absolute() || program.as_os_str().len() > 16 << 10 {
            return Err(MediaError::Unsupported("scene-render program path must be absolute and at most 16 KiB".into()));
        }
        let cwd = program.parent().ok_or_else(|| MediaError::Unsupported("scene-render program has no parent".into()))?;
        let _global = source::process_lock()?;
        let output = runner.run(program, &args::version_args(), cwd, timeout.min(Duration::from_secs(30)))?;
        process::check(&output)?;
        let version = discover::parse_version(&output.stdout_tail).ok_or_else(|| MediaError::Unsupported("scene-render returned an invalid version".into()))?;
        status.version = Some(format!("{}.{}.{}", version.major, version.minor, version.patch));
        if version < discover::MIN_VERSION {
            return Err(MediaError::Unsupported("scene-render version is too old; install 0.5.0 or newer".into()));
        }
        let output = runner.run(program, &args::capabilities_args(), cwd, timeout.min(Duration::from_secs(30)))?;
        process::check(&output)?;
        let capabilities = discover::parse_capabilities(&output.stdout_tail).map_err(MediaError::Unsupported)?;
        status.schema = Some(capabilities.schema);
        Ok(())
    })();
    match result {
        Ok(()) => status.found = true,
        Err(error) => status.reason = Some(format!("scene-render unavailable: {error}")),
    }
    status
}

#[cfg(target_arch = "wasm32")]
fn probe_with(_program: Option<&Path>, _runner: &dyn process::Runner, _timeout: Duration) -> Status {
    unavailable("scene-render is unavailable on wasm")
}

/// Locate and probe the renderer, caching by resolved path, file length and modification time.
/// The cache holds at most 128 executables; probes have a 30-second deadline each.
#[cfg(not(target_arch = "wasm32"))]
pub fn probe(config: &Config) -> Status {
    cached_probe(config).0
}

#[cfg(not(target_arch = "wasm32"))]
fn cached_probe(config: &Config) -> (Status, Option<PathBuf>) {
    use std::{
        collections::HashMap,
        sync::{LazyLock, Mutex, PoisonError},
        time::SystemTime,
    };
    type Identity = (PathBuf, u64, Option<SystemTime>);
    static CACHE: LazyLock<Mutex<HashMap<Identity, Status>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    let Some(program) = discover::locate_system(config) else {
        return (probe_with(None, &process::ProcessRunner, Duration::from_secs(30)), None);
    };
    let resolved = match program.canonicalize() {
        Ok(path) => path,
        Err(error) => return (unavailable(format!("scene-render path: {error}")), None),
    };
    let metadata = match std::fs::metadata(&resolved) {
        Ok(metadata) => metadata,
        Err(error) => return (unavailable(format!("scene-render metadata: {error}")), None),
    };
    let identity = (resolved.clone(), metadata.len(), metadata.modified().ok());
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(status) = cache.get(&identity) {
        return (status.clone(), Some(resolved));
    }
    let status = probe_with(Some(&resolved), &process::ProcessRunner, Duration::from_secs(30));
    if filmcraft_media::cancel::cancelled() {
        return (status, Some(resolved));
    }
    cache.retain(|(path, _, _), _| path != &resolved);
    if cache.len() >= 128 {
        cache.clear();
    }
    cache.insert(identity, status.clone());
    (status, Some(resolved))
}

/// External processes are unavailable on wasm; scenes open as placeholders.
#[cfg(target_arch = "wasm32")]
pub fn probe(_config: &Config) -> Status {
    cached_probe(_config).0
}

#[cfg(target_arch = "wasm32")]
fn cached_probe(_config: &Config) -> (Status, Option<PathBuf>) {
    (probe_with(None, &process::ProcessRunner, Duration::ZERO), None)
}

#[cfg(all(test, unix))]
mod source_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quality_names_match_the_cli() {
        assert_eq!([Quality::Draft, Quality::Preview, Quality::Final].map(Quality::as_str), ["draft", "preview", "final"]);
    }
}
