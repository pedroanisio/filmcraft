//! Pure renderer discovery and bounded probe-output parsing.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// Renderer preference; absent means environment and PATH discovery.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Explicit executable path.
    pub program: Option<PathBuf>,
}

const MAX_PATH_BYTES: usize = 64 << 10;
const MAX_ENV_BYTES: usize = 1 << 20;

/// Locate a renderer, trying the explicit preference, FILMCRAFT_SCENE_RENDER, then PATH.
/// Candidates are capped at 64 KiB, environment values at 1 MiB and PATH at 4096 entries.
/// File existence is injected; this function neither executes nor validates the renderer.
pub fn locate(config: &Config, env: impl Fn(&str) -> Option<OsString>, is_file: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let usable = |p: &Path| !p.as_os_str().is_empty() && p.as_os_str().len() <= MAX_PATH_BYTES && is_file(p);
    if let Some(path) = config.program.as_ref().filter(|p| usable(p)) {
        return Some(path.clone());
    }
    if let Some(value) = env("FILMCRAFT_SCENE_RENDER").filter(|v| v.len() <= MAX_ENV_BYTES) {
        let path = Path::new(&value);
        if usable(path) {
            return Some(path.to_owned());
        }
    }
    // std's split_paths is unsupported on wasm; explicit injected candidates still work.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(value) = env("PATH").filter(|v| v.len() <= MAX_ENV_BYTES) {
        let binary = if cfg!(windows) { "scene-render.exe" } else { "scene-render" };
        for dir in std::env::split_paths(&value).take(4096) {
            if dir.as_os_str().len() > MAX_PATH_BYTES.saturating_sub(binary.len().saturating_add(1)) {
                continue;
            }
            let path = dir.join(binary);
            if usable(&path) {
                return Some(path);
            }
        }
    }
    None
}

/// Locate the installed renderer through the real environment and filesystem; unavailable on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub fn locate_system(config: &Config) -> Option<PathBuf> {
    locate(config, |key| std::env::var_os(key), Path::is_file)
}

/// The external renderer is unavailable on wasm.
#[cfg(target_arch = "wasm32")]
pub fn locate_system(_config: &Config) -> Option<PathBuf> {
    None
}

/// Comparable renderer release version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Version {
    /// Major release.
    pub major: u32,
    /// Minor release.
    pub minor: u32,
    /// Patch release.
    pub patch: u32,
}

/// First renderer release supporting this bridge's CLI contract.
pub const MIN_VERSION: Version = Version { major: 0, minor: 5, patch: 0 };

/// Parse a `scene-render X.Y.Z` line, accepting trailing text; input capped at 64 KiB.
pub fn parse_version(stdout: &str) -> Option<Version> {
    if stdout.len() > 64 << 10 {
        return None;
    }
    let line = stdout.lines().next()?;
    let mut words = line.split_ascii_whitespace();
    if words.next()? != "scene-render" {
        return None;
    }
    let mut parts = words.next()?.split('.');
    let term = |s: &str| -> Option<u32> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    let major = term(parts.next()?)?;
    let minor = term(parts.next()?)?;
    let patch = term(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some(Version { major, minor, patch })
}

/// Renderer and schema versions from the capability manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Renderer release string.
    pub engine_version: String,
    /// Supported sr-core schema version.
    pub schema: String,
}

/// Parse format version 1 of the capability manifest, capped at 8 MiB.
pub fn parse_capabilities(json: &str) -> Result<Capabilities, String> {
    if json.len() > 8 << 20 {
        return Err("scene-render capabilities exceed 8 MiB".into());
    }
    #[derive(Deserialize)]
    struct Engine {
        version: String,
    }
    #[derive(Deserialize)]
    struct Manifest {
        format: String,
        engine: Engine,
        schema: String,
    }
    let manifest: Manifest = serde_json::from_str(json).map_err(|e| format!("invalid scene-render capabilities: {e}"))?;
    if manifest.format != "scene-render-capabilities/1" {
        return Err("unsupported scene-render capabilities format (expected scene-render-capabilities/1)".into());
    }
    for (name, value) in [("engine.version", &manifest.engine.version), ("schema", &manifest.schema)] {
        if value.trim().is_empty() || value.len() > 256 {
            return Err(format!("scene-render capabilities {name} must contain 1..=256 bytes"));
        }
    }
    Ok(Capabilities { engine_version: manifest.engine.version, schema: manifest.schema })
}

/// Serializable result of `scene.renderer.status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Whether a compatible renderer was found.
    pub found: bool,
    /// Resolved executable path.
    pub path: Option<String>,
    /// Renderer release string.
    pub version: Option<String>,
    /// Supported schema version.
    pub schema: Option<String>,
    /// Explanation when unavailable.
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary() -> &'static str {
        if cfg!(windows) { "scene-render.exe" } else { "scene-render" }
    }

    #[test]
    fn discovery_precedence_and_missing_candidate_fallbacks() {
        let explicit = PathBuf::from("preferred");
        let override_path = PathBuf::from("override");
        let path_binary = PathBuf::from("bin").join(binary());
        let path = std::env::join_paths([Path::new("missing"), Path::new("bin")]).unwrap();
        let env = |key: &str| match key {
            "FILMCRAFT_SCENE_RENDER" => Some(override_path.clone().into_os_string()),
            "PATH" => Some(path.clone()),
            _ => None,
        };
        let config = Config { program: Some(explicit.clone()) };
        assert_eq!(locate(&config, env, |_| true), Some(explicit.clone()));
        assert_eq!(locate(&config, env, |p| p == override_path || p == path_binary), Some(override_path.clone()));
        assert_eq!(locate(&config, env, |p| p == path_binary), Some(path_binary));
        assert_eq!(locate(&config, env, |_| false), None);
        assert_eq!(locate(&Config::default(), |_| None, |_| true), None);
    }

    #[test]
    fn discovery_preserves_path_order_and_bounds_environment() {
        let path = std::env::join_paths([Path::new("first"), Path::new("second")]).unwrap();
        assert_eq!(locate(&Config::default(), |key| (key == "PATH").then(|| path.clone()), |_| true), Some(PathBuf::from("first").join(binary())));
        let huge = OsString::from("x".repeat((1 << 20) + 1));
        assert_eq!(locate(&Config::default(), |_| Some(huge.clone()), |_| true), None);
    }

    #[test]
    fn version_line_accepts_release_and_trailing_text() {
        assert_eq!(parse_version("scene-render 0.5.0\n"), Some(MIN_VERSION));
        assert_eq!(parse_version("scene-render 12.34.56 build abc\n"), Some(Version { major: 12, minor: 34, patch: 56 }));
        assert!(Version { major: 1, minor: 0, patch: 0 } > MIN_VERSION);
    }

    #[test]
    fn version_rejects_empty_garbage_overflow_and_oversize() {
        for bad in [
            "",
            "garbage",
            "other 0.5.0",
            "scene-render",
            "scene-render 0.5",
            "scene-render -1.5.0",
            "scene-render 0.5.0.1",
            "scene-render 0.5.x",
            "scene-render 4294967296.0.0",
            "scene-render é.5.0",
        ] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
        assert_eq!(parse_version(&format!("scene-render 0.5.0 {}", "x".repeat(64 << 10))), None);
    }

    const CAPABILITIES: &str =
        r#"{"format":"scene-render-capabilities/1","engine":{"name":"rs-scene-render","version":"0.5.0"},"schema":"1.7.0","entries":[]}"#;

    #[test]
    fn capabilities_read_required_fields_and_ignore_extensions() {
        assert_eq!(parse_capabilities(CAPABILITIES).unwrap(), Capabilities { engine_version: "0.5.0".into(), schema: "1.7.0".into() });
    }

    #[test]
    fn capabilities_reject_bad_format_shape_and_gigantic_input() {
        for bad in [
            "".into(),
            "{".into(),
            CAPABILITIES.replace("capabilities/1", "capabilities/2"),
            "{}".into(),
            CAPABILITIES.replace("\"1.7.0\"", "null"),
            CAPABILITIES.replace("0.5.0", ""),
            CAPABILITIES.replace("1.7.0", ""),
            " ".repeat((8 << 20) + 1),
        ] {
            assert!(parse_capabilities(&bad).is_err(), "accepted {} bytes", bad.len());
        }
    }

    #[test]
    fn parser_mutations_do_not_panic() {
        for i in 0..CAPABILITIES.len() {
            assert!(std::panic::catch_unwind(|| parse_capabilities(&CAPABILITIES[..i])).is_ok());
            let mut bytes = CAPABILITIES.as_bytes().to_vec();
            bytes[i] ^= 0x7f;
            let changed = String::from_utf8_lossy(&bytes);
            assert!(std::panic::catch_unwind(|| parse_capabilities(&changed)).is_ok());
        }
        assert!(parse_capabilities(&CAPABILITIES[..CAPABILITIES.len() - 1]).is_err());
    }

    #[test]
    fn config_and_status_serialize() {
        assert_eq!(serde_json::from_str::<Config>("{}").unwrap(), Config::default());
        let status = Status { found: false, reason: Some("not installed".into()), ..Status::default() };
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({"found":false,"path":null,"version":null,"schema":null,"reason":"not installed"})
        );
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn system_discovery_unavailable_on_wasm() {
        assert_eq!(locate_system(&Config::default()), None);
    }
}
