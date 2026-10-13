//! The on-disk frame cache under the media cache directory, keyed by renderer, quality, document and assets.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

use crate::Quality;
use filmcraft_bitstream::sha256::{Sha256, sha256, to_hex};
use filmcraft_media::{MediaError, Result};
use std::ops::Range;
use std::path::{Path, PathBuf};

const MAX_ASSETS: usize = 4096;
const MAX_REFERENCE_BYTES: usize = 16 * 1024;

/// A local asset's identity and filesystem metadata; absent fields mean unavailable metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetStamp {
    /// Resolved absolute local path (hashed if non-UTF-8), or the unresolved reference.
    pub reference: String,
    /// File size in bytes, when readable.
    pub len: Option<u64>,
    /// Signed nanoseconds since the Unix epoch, when readable.
    pub modified_ns: Option<i128>,
}

/// Stamp the first 4096 references. Callers must reject documents with more references.
/// References over 16 KiB use a `sha256:` identity and unknown metadata to bound storage.
/// On wasm all metadata is unknown. Local identities include the resolved absolute path.
pub fn stamp_assets(base_dir: &Path, refs: &[String]) -> Vec<AssetStamp> {
    refs.iter()
        .take(MAX_ASSETS)
        .map(|reference| {
            if reference.len() > MAX_REFERENCE_BYTES {
                return AssetStamp { reference: format!("sha256:{}", to_hex(&sha256(reference.as_bytes()))), len: None, modified_ns: None };
            }
            let stamp = AssetStamp { reference: reference.clone(), len: None, modified_ns: None };
            #[cfg(not(target_arch = "wasm32"))]
            {
                native::stamp(base_dir, stamp)
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = base_dir;
                stamp
            }
        })
        .collect()
}

/// SHA-256 of a versioned, length-framed renderer/document/asset identity.
/// All input bytes are streamed into the hash without an input-sized scratch allocation.
pub fn key(renderer_version: &str, quality: Quality, doc: &[u8], assets: &[AssetStamp]) -> String {
    let mut hash = Sha256::new();
    field(&mut hash, b"filmcraft.scene-render.frames");
    field(&mut hash, &1u32.to_be_bytes());
    field(&mut hash, renderer_version.as_bytes());
    field(&mut hash, quality.as_str().as_bytes());
    field(&mut hash, doc);
    field(&mut hash, &(assets.len() as u64).to_be_bytes());
    for asset in assets {
        field(&mut hash, asset.reference.as_bytes());
        match asset.len {
            Some(n) => field(&mut hash, &n.to_be_bytes()),
            None => field(&mut hash, b""),
        }
        match asset.modified_ns {
            Some(n) => field(&mut hash, &n.to_be_bytes()),
            None => field(&mut hash, b""),
        }
    }
    to_hex(&hash.finish())
}

fn field(hash: &mut Sha256, bytes: &[u8]) {
    hash.update(&(bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

/// Disk cache rooted at the media cache's `Scene Renders` directory.
#[derive(Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Open or create the scene cache; filesystem errors and wasm return an error.
    pub fn new(media_cache_dir: &Path) -> Result<Self> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if media_cache_dir.as_os_str().len() > MAX_REFERENCE_BYTES {
                return Err(MediaError::Io("scene cache path exceeds 16 KiB".into()));
            }
            let root = media_cache_dir.join("Scene Renders");
            std::fs::create_dir_all(&root).map_err(|e| native::io(&root, e))?;
            native::directory(&root)?;
            Ok(Self { root: root.canonicalize().map_err(|e| native::io(&root, e))? })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = media_cache_dir;
            Err(unsupported())
        }
    }
    /// Locate a frame using the renderer's minimum-six-digit numbering.
    pub fn frame_path(&self, key: &str, quality: Quality, k: u64) -> Result<PathBuf> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            validate_key(key)?;
            Ok(self.root.join(key).join(quality.as_str()).join(format!("{k:06}.png")))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (self, key, quality, k);
            Err(unsupported())
        }
    }
    /// Test whether a complete frame is present.
    pub fn has_frame(&self, key: &str, quality: Quality, k: u64) -> Result<bool> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = self.frame_path(key, quality, k)?;
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) => Ok(metadata.is_file()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(native::io(&path, e)),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (self, key, quality, k);
            Err(unsupported())
        }
    }
    /// Create a unique staging directory for one chunk.
    pub fn begin_chunk(&self, key: &str, quality: Quality) -> Result<ChunkDir> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            native::begin(&self.root, key, quality)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (self, key, quality);
            Err(unsupported())
        }
    }
    /// Remove the cache and return its recorded file bytes.
    pub fn clear(&self) -> Result<u64> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let bytes = self.try_size()?;
            native::remove(&self.root)?;
            Ok(bytes)
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err(unsupported())
        }
    }
    /// Inspect all file bytes, including staged frames, without following symlinks.
    /// Traversal exceeding 64 directory levels or one million entries returns an error.
    pub fn try_size(&self) -> Result<u64> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            native::tree_size(&self.root)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = &self.root;
            Err(unsupported())
        }
    }
    /// Best-effort size; returns zero if the cache cannot be inspected or on wasm.
    pub fn size(&self) -> u64 {
        self.try_size().unwrap_or_default()
    }
}

/// A private staging directory; dropping it attempts to remove unpublished files.
#[derive(Debug)]
pub struct ChunkDir {
    path: PathBuf,
    #[cfg(not(target_arch = "wasm32"))]
    destination: PathBuf,
    active: bool,
}
impl ChunkDir {
    /// Output pattern passed directly to scene-render.
    pub fn pattern(&self) -> PathBuf {
        self.path.join("%06d.png")
    }
    /// Validate every expected regular file before publishing any by atomic rename.
    /// Ranges are end-exclusive and limited to 65536 frames. Validation failure publishes
    /// nothing; a later rename error can leave a prefix of complete frames published.
    /// The renderer must have exited before this method is called.
    pub fn commit(self, frames: Range<u64>, validate: impl Fn(&Path) -> Result<()>) -> Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            native::commit(&self, frames, validate)?;
            self.discard()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (frames, validate);
            Err(unsupported())
        }
    }
    /// Remove staging files and report cleanup errors; Drop can only attempt cleanup.
    pub fn discard(mut self) -> Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            native::tree_size(&self.path)?;
            native::remove(&self.path)?;
            self.active = false;
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.active = false;
            Err(unsupported())
        }
    }
}

impl Drop for ChunkDir {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.active && native::tree_size(&self.path).is_ok() {
            let _ = native::remove(&self.path);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn validate_key(key: &str) -> Result<()> {
    if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(MediaError::Io("scene cache key must contain exactly 64 hexadecimal characters".into()));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn unsupported() -> MediaError {
    MediaError::Unsupported("scene frame disk cache is unavailable on wasm".into())
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::fs;

    static NEXT_CHUNK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    pub(super) fn begin(root: &Path, key: &str, quality: Quality) -> Result<ChunkDir> {
        use std::sync::atomic::Ordering;
        validate_key(key)?;
        fs::create_dir_all(root).map_err(|e| io(root, e))?;
        directory(root)?;
        let parent = root.join(key);
        fs::create_dir_all(&parent).map_err(|e| io(&parent, e))?;
        directory(&parent)?;
        for _ in 0..128 {
            let n = NEXT_CHUNK
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .map_err(|_| MediaError::Io("scene cache temporary directory counter exhausted".into()))?;
            let path = parent.join(format!(".tmp-{}-{n}", std::process::id()));
            let mut builder = fs::DirBuilder::new();
            builder.recursive(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(ChunkDir { path, destination: parent.join(quality.as_str()), active: true }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io(&path, e)),
            }
        }
        Err(MediaError::Io("could not create a unique scene chunk directory after 128 attempts".into()))
    }

    pub(super) fn commit(chunk: &ChunkDir, frames: Range<u64>, validate: impl Fn(&Path) -> Result<()>) -> Result<()> {
        let count = frames
            .end
            .checked_sub(frames.start)
            .filter(|n| *n <= 65536)
            .ok_or_else(|| MediaError::Io("scene chunk range must be ordered and contain at most 65536 frames".into()))?;
        directory(&chunk.path)?;
        for k in frames.clone() {
            let path = chunk.path.join(format!("{k:06}.png"));
            let metadata = fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
            if !metadata.is_file() {
                return Err(MediaError::Decode(format!("{}: expected a regular rendered frame", path.display())));
            }
            validate(&path)?;
        }
        if count == 0 {
            return Ok(());
        }
        if let Some(parent) = chunk.destination.parent() {
            directory(parent)?;
        }
        fs::create_dir_all(&chunk.destination).map_err(|e| io(&chunk.destination, e))?;
        directory(&chunk.destination)?;
        for k in frames {
            let filename = format!("{k:06}.png");
            let from = chunk.path.join(&filename);
            let to = chunk.destination.join(&filename);
            fs::rename(&from, &to).map_err(|e| MediaError::Io(format!("publish {} to {}: {e}", from.display(), to.display())))?;
        }
        Ok(())
    }

    pub(super) fn remove(path: &Path) -> Result<()> {
        match fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(path, e)),
        }
    }

    pub(super) fn tree_size(root: &Path) -> Result<u64> {
        match fs::symlink_metadata(root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(io(root, e)),
            Ok(metadata) if !metadata.is_dir() => return Err(MediaError::Io(format!("{}: expected a real cache directory", root.display()))),
            Ok(_) => {}
        }
        walk(root, 0, &mut 1_000_000)
    }

    fn walk(path: &Path, depth: u32, remaining: &mut u64) -> Result<u64> {
        if depth >= 64 {
            return Err(MediaError::Io("scene cache exceeds 64 directory levels".into()));
        }
        let mut bytes = 0u64;
        for entry in fs::read_dir(path).map_err(|e| io(path, e))? {
            *remaining = remaining.checked_sub(1).ok_or_else(|| MediaError::Io("scene cache exceeds one million entries".into()))?;
            let entry = entry.map_err(|e| io(path, e))?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| io(&path, e))?;
            if metadata.is_dir() {
                bytes = bytes.saturating_add(walk(&path, depth + 1, remaining)?);
            } else if metadata.is_file() {
                bytes = bytes.saturating_add(metadata.len());
            }
        }
        Ok(bytes)
    }

    pub(super) fn io(path: &Path, error: std::io::Error) -> MediaError {
        MediaError::Io(format!("{}: {error}", path.display()))
    }
    pub(super) fn directory(path: &Path) -> Result<()> {
        let metadata = fs::symlink_metadata(path).map_err(|e| io(path, e))?;
        if !metadata.is_dir() {
            return Err(MediaError::Io(format!("{}: expected a real cache directory", path.display())));
        }
        Ok(())
    }
    pub(super) fn stamp(base: &Path, mut stamp: AssetStamp) -> AssetStamp {
        let Some(path) = resolve(base, &stamp.reference) else { return stamp };
        let Ok(path) = path.canonicalize() else { return stamp };
        if path.as_os_str().len() > MAX_REFERENCE_BYTES {
            return stamp;
        }
        stamp.reference = match path.to_str() {
            Some(path) => path.to_owned(),
            None => format!("path-sha256:{}", to_hex(&sha256(path.as_os_str().as_encoded_bytes()))),
        };
        if let Ok(metadata) = fs::metadata(&path)
            && metadata.is_file()
        {
            stamp.len = Some(metadata.len());
            stamp.modified_ns = metadata.modified().ok().map(|time| match time.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => d.as_nanos() as i128,
                Err(e) => -(e.duration().as_nanos() as i128),
            });
        }
        stamp
    }
    fn resolve(base: &Path, reference: &str) -> Option<PathBuf> {
        let uri = reference.trim();
        if let Some(rest) = uri.strip_prefix("file://") {
            return Some(PathBuf::from(decode(rest.strip_prefix("localhost").unwrap_or(rest))));
        }
        if let Some((scheme, rest)) = uri.split_once(':') {
            let is_scheme = scheme.len() >= 2
                && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
            if is_scheme {
                return scheme.eq_ignore_ascii_case("file").then(|| PathBuf::from(decode(rest)));
            }
        }
        let path = PathBuf::from(decode(uri));
        if path.is_absolute() {
            Some(path)
        } else if base.as_os_str().len() <= MAX_REFERENCE_BYTES {
            Some(base.join(path))
        } else {
            None
        }
    }
    fn decode(input: &str) -> String {
        let mut bytes = input.bytes();
        let mut out = Vec::with_capacity(input.len());
        while let Some(b) = bytes.next() {
            if b == b'%' {
                let mut lookahead = bytes.clone();
                if let (Some(a), Some(b)) = (lookahead.next(), lookahead.next())
                    && let (Some(a), Some(b)) = (char::from(a).to_digit(16), char::from(b).to_digit(16))
                {
                    out.push((a * 16 + b) as u8);
                    bytes = lookahead;
                    continue;
                }
            }
            out.push(b);
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("filmcraft-store-tests-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn cache_key() -> String {
        "ab".repeat(32)
    }
    fn asset() -> AssetStamp {
        AssetStamp { reference: "asset.png".into(), len: Some(12), modified_ns: Some(34) }
    }

    #[test]
    fn key_is_stable_lowercase_sha256() {
        let k = key("0.5.0", Quality::Preview, b"doc", &[asset()]);
        assert_eq!(k.len(), 64);
        assert!(k.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(k, key("0.5.0", Quality::Preview, b"doc", &[asset()]));
    }
    #[test]
    fn key_changes_for_every_input() {
        let baseline = key("0.5.0", Quality::Preview, b"doc", &[asset()]);
        let mut variants = vec![
            key("0.5.1", Quality::Preview, b"doc", &[asset()]),
            key("0.5.0", Quality::Final, b"doc", &[asset()]),
            key("0.5.0", Quality::Draft, b"doc", &[asset()]),
            key("0.5.0", Quality::Preview, b"doc!", &[asset()]),
            key("0.5.0", Quality::Preview, b"doc", &[]),
        ];
        for changed in [
            AssetStamp { reference: "other".into(), ..asset() },
            AssetStamp { len: Some(13), ..asset() },
            AssetStamp { modified_ns: Some(-34), ..asset() },
            AssetStamp { len: None, ..asset() },
            AssetStamp { modified_ns: None, ..asset() },
        ] {
            variants.push(key("0.5.0", Quality::Preview, b"doc", &[changed]));
        }
        for variant in variants {
            assert_ne!(baseline, variant);
        }
        assert_ne!(
            key("v", Quality::Draft, b"", &[asset(), AssetStamp { len: None, ..asset() }]),
            key("v", Quality::Draft, b"", &[AssetStamp { len: None, ..asset() }, asset()])
        );
    }
    #[test]
    fn key_field_boundaries_and_none_are_unambiguous() {
        assert_ne!(key("ab", Quality::Preview, b"c", &[]), key("a", Quality::Preview, b"bc", &[]));
        let refs = |a: &str, b: &str| {
            [AssetStamp { reference: a.into(), len: None, modified_ns: None }, AssetStamp { reference: b.into(), len: None, modified_ns: None }]
        };
        assert_ne!(key("v", Quality::Draft, b"", &refs("ab", "c")), key("v", Quality::Draft, b"", &refs("a", "bc")));
        assert_ne!(
            key("v", Quality::Draft, b"", &[AssetStamp { len: None, ..asset() }]),
            key("v", Quality::Draft, b"", &[AssetStamp { len: Some(0), ..asset() }])
        );
    }
    #[test]
    fn stamps_resolve_local_paths_urls_escapes_and_parents() {
        let t = Temp::new();
        fs::create_dir(t.0.join("sub")).unwrap();
        let file = t.0.join("a ü.png");
        fs::write(&file, b"png").unwrap();
        let refs = vec![
            "a%20%C3%BC.png".into(),
            "sub/../a ü.png".into(),
            format!("file://{}", file.display()),
            format!("file://localhost{}", file.display()),
            format!("file:{}", file.display()),
        ];
        let stamps = stamp_assets(&t.0, &refs);
        assert_eq!(stamps.len(), refs.len());
        for stamp in stamps {
            assert_eq!(stamp.reference, file.canonicalize().unwrap().to_string_lossy());
            assert_eq!(stamp.len, Some(3));
            assert!(stamp.modified_ns.is_some());
        }
    }
    #[test]
    fn stamps_missing_remote_directory_and_invalid_refs_are_unknown() {
        let t = Temp::new();
        let refs: Vec<String> = ["missing", "https://example.com/a", "data:image/png;base64,x", ".", "a\0b"].map(str::to_owned).into();
        let stamps = stamp_assets(&t.0, &refs);
        assert_eq!(stamps.len(), refs.len());
        assert!(stamps.iter().all(|s| s.len.is_none() && s.modified_ns.is_none()));
    }
    #[test]
    fn stamps_are_bounded_and_asset_changes_invalidate_key() {
        let t = Temp::new();
        let refs = vec!["missing".into(); 4097];
        assert_eq!(stamp_assets(&t.0, &refs).len(), 4096);
        let refs = vec!["a".into()];
        fs::write(t.0.join("a"), b"a").unwrap();
        let first = key("v", Quality::Draft, b"", &stamp_assets(&t.0, &refs));
        fs::write(t.0.join("a"), b"changed").unwrap();
        assert_ne!(first, key("v", Quality::Draft, b"", &stamp_assets(&t.0, &refs)));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_asset_paths_cannot_share_an_identity() {
        use std::os::unix::ffi::OsStringExt;
        let t = Temp::new();
        let mut stamps = Vec::new();
        for byte in [0xfe, 0xff] {
            let base = t.0.join(std::ffi::OsString::from_vec(vec![byte]));
            fs::create_dir(&base).unwrap();
            fs::write(base.join("a"), b"same").unwrap();
            let file = fs::OpenOptions::new().write(true).open(base.join("a")).unwrap();
            file.set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH)).unwrap();
            stamps.push(stamp_assets(&base, &["a".into()]));
        }
        assert_ne!(stamps[0][0].reference, stamps[1][0].reference);
        assert_ne!(key("v", Quality::Draft, b"doc", &stamps[0]), key("v", Quality::Draft, b"doc", &stamps[1]));
    }

    #[test]
    fn asset_reference_mutations_remain_bounded_and_never_panic() {
        let t = Temp::new();
        let original = b"file://localhost/a%20b/%C3%BC.png";
        let mut refs = Vec::new();
        for n in 0..original.len() {
            refs.push(String::from_utf8_lossy(&original[..n]).into_owned());
            for mask in [1, 0x80, 0xff] {
                let mut bytes = original.to_vec();
                bytes[n] ^= mask;
                refs.push(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        let stamps = std::panic::catch_unwind(|| stamp_assets(&t.0, &refs)).unwrap();
        assert_eq!(stamps.len(), refs.len());
        let huge = "x".repeat(100_000);
        let stamps = stamp_assets(&t.0, &[huge.clone(), format!("{huge}y")]);
        assert!(stamps.iter().all(|s| s.reference.len() < 100 && s.len.is_none() && s.modified_ns.is_none()));
        assert_ne!(stamps[0].reference, stamps[1].reference);
    }
    #[test]
    fn paths_match_renderer_numbering_including_u64_max() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        for (k, filename) in [(0, "000000.png"), (42, "000042.png"), (1_000_000, "1000000.png"), (u64::MAX, "18446744073709551615.png")] {
            assert_eq!(s.frame_path(&cache_key(), Quality::Preview, k).unwrap(), t.0.join("Scene Renders").join(cache_key()).join("preview").join(filename));
        }
    }
    #[test]
    fn paths_reject_traversal_non_hex_and_wrong_lengths() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        for k in ["../".into(), "z".repeat(64), "a".repeat(63), "a".repeat(65), String::new()] {
            assert!(s.frame_path(&k, Quality::Draft, 0).is_err(), "{k}");
        }
    }
    #[test]
    fn root_that_is_a_file_returns_an_error() {
        let t = Temp::new();
        fs::write(t.0.join("Scene Renders"), b"sentinel").unwrap();
        assert!(Store::new(&t.0).is_err());
        assert_eq!(fs::read(t.0.join("Scene Renders")).unwrap(), b"sentinel");
    }

    #[test]
    fn begin_chunks_are_unique_and_use_the_renderer_pattern() {
        let t = Temp::new();
        let a = Store::new(&t.0).unwrap();
        let b = Store::new(&t.0).unwrap();
        let a = a.begin_chunk(&cache_key(), Quality::Draft).unwrap();
        let b = b.begin_chunk(&cache_key(), Quality::Final).unwrap();
        assert_ne!(a.pattern(), b.pattern());
        for chunk in [a, b] {
            let pattern = chunk.pattern();
            assert_eq!(pattern.file_name().unwrap(), "%06d.png");
            let dir = pattern.parent().unwrap();
            assert!(dir.is_dir());
            assert_eq!(dir.parent().unwrap(), t.0.join("Scene Renders").join(cache_key()));
            assert!(dir.file_name().unwrap().to_string_lossy().starts_with(&format!(".tmp-{}-", std::process::id())));
        }
    }

    #[test]
    fn begin_rejects_invalid_keys_and_damaged_roots() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        let chunk = s.begin_chunk(&cache_key(), Quality::Preview).unwrap();
        assert!(chunk.pattern().parent().unwrap().is_dir());
        for key in ["../".into(), "g".repeat(64)] {
            assert!(s.begin_chunk(&key, Quality::Draft).is_err());
        }
        fs::remove_dir_all(t.0.join("Scene Renders")).unwrap();
        fs::write(t.0.join("Scene Renders"), b"sentinel").unwrap();
        assert!(s.begin_chunk(&cache_key(), Quality::Draft).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn read_only_root_returns_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        s.begin_chunk(&cache_key(), Quality::Draft).unwrap();
        let root = t.0.join("Scene Renders");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o500)).unwrap();
        let result = s.begin_chunk(&"cd".repeat(32), Quality::Preview);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
    }
    fn write_frame(chunk: &ChunkDir, k: u64, bytes: &[u8]) {
        fs::write(chunk.pattern().parent().unwrap().join(format!("{k:06}.png")), bytes).unwrap();
    }
    fn validate_png(path: &Path) -> Result<()> {
        let bytes = fs::read(path).map_err(|e| MediaError::Io(e.to_string()))?;
        image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).map_err(|e| MediaError::Decode(e.to_string()))?;
        Ok(())
    }
    fn png() -> Vec<u8> {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(2, 2, image::Rgba([23, 45, 67, 255])).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }
    #[test]
    fn chunk_commit_publishes_pngs_then_removes_staging() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        let chunk = s.begin_chunk(&cache_key(), Quality::Preview).unwrap();
        let tmp = chunk.pattern().parent().unwrap().to_path_buf();
        for k in 12..15 {
            write_frame(&chunk, k, &png());
        }
        assert!(!s.has_frame(&cache_key(), Quality::Preview, 12).unwrap());
        chunk.commit(12..15, validate_png).unwrap();
        for k in 12..15 {
            assert!(s.has_frame(&cache_key(), Quality::Preview, k).unwrap());
            validate_png(&s.frame_path(&cache_key(), Quality::Preview, k).unwrap()).unwrap();
        }
        assert!(!tmp.exists());
    }
    #[test]
    fn missing_or_invalid_frame_publishes_nothing_and_preserves_existing() {
        for invalid in [false, true] {
            let t = Temp::new();
            let s = Store::new(&t.0).unwrap();
            let old = s.frame_path(&cache_key(), Quality::Preview, 12).unwrap();
            fs::create_dir_all(old.parent().unwrap()).unwrap();
            fs::write(&old, b"existing").unwrap();
            let chunk = s.begin_chunk(&cache_key(), Quality::Preview).unwrap();
            let tmp = chunk.pattern().parent().unwrap().to_path_buf();
            write_frame(&chunk, 12, &png());
            if invalid {
                write_frame(&chunk, 13, b"bad png");
            }
            assert!(chunk.commit(12..14, validate_png).is_err());
            assert_eq!(fs::read(old).unwrap(), b"existing");
            assert!(!s.frame_path(&cache_key(), Quality::Preview, 13).unwrap().exists());
            assert!(!tmp.exists());
        }
    }
    #[test]
    fn chunk_drop_removes_unpublished_files() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        let chunk = s.begin_chunk(&cache_key(), Quality::Draft).unwrap();
        let tmp = chunk.pattern().parent().unwrap().to_path_buf();
        write_frame(&chunk, 0, b"partial");
        drop(chunk);
        assert!(!tmp.exists());
    }
    #[test]
    fn has_frame_checks_disk_and_rejects_bad_keys() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        let p = s.frame_path(&cache_key(), Quality::Final, u64::MAX).unwrap();
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"complete").unwrap();
        assert!(s.has_frame(&cache_key(), Quality::Final, u64::MAX).unwrap());
        assert!(!s.has_frame(&cache_key(), Quality::Preview, u64::MAX).unwrap());
        assert!(s.has_frame("../", Quality::Final, 0).is_err());
    }
    #[test]
    fn size_and_clear_count_bytes_and_allow_reuse() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        assert_eq!(s.size(), 0);
        let chunk = s.begin_chunk(&cache_key(), Quality::Draft).unwrap();
        write_frame(&chunk, 0, b"abcd");
        write_frame(&chunk, 1, b"xyz");
        assert_eq!(s.size(), 7);
        assert_eq!(s.clear().unwrap(), 7);
        assert!(!t.0.join("Scene Renders").exists());
        assert_eq!(s.clear().unwrap(), 0);
        assert_eq!(s.size(), 0);
        drop(chunk);
        assert!(s.begin_chunk(&cache_key(), Quality::Preview).is_ok());
    }
    #[test]
    fn hostile_frame_ranges_are_rejected_before_validation() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        for frames in [0..u64::MAX, Range { start: 9, end: 4 }] {
            let chunk = s.begin_chunk(&cache_key(), Quality::Draft).unwrap();
            assert!(chunk.commit(frames, |_| panic!("hostile range reached validation")).is_err());
        }
    }
    #[test]
    fn empty_chunk_and_large_frame_numbers_are_supported() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        let chunk = s.begin_chunk(&cache_key(), Quality::Draft).unwrap();
        let tmp = chunk.pattern().parent().unwrap().to_path_buf();
        chunk.commit(0..0, |_| panic!("empty chunk reached validation")).unwrap();
        assert!(!tmp.exists());
        let chunk = s.begin_chunk(&cache_key(), Quality::Final).unwrap();
        write_frame(&chunk, u64::MAX - 1, &png());
        chunk.commit(u64::MAX - 1..u64::MAX, validate_png).unwrap();
        assert!(s.has_frame(&cache_key(), Quality::Final, u64::MAX - 1).unwrap());
    }
    #[test]
    fn validation_finishes_before_any_frame_is_published() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        let chunk = s.begin_chunk(&cache_key(), Quality::Preview).unwrap();
        for k in 0..3 {
            write_frame(&chunk, k, &png());
        }
        let calls = std::cell::Cell::new(0);
        chunk
            .commit(0..3, |path| {
                calls.set(calls.get() + 1);
                for k in 0..3 {
                    assert!(!s.frame_path(&cache_key(), Quality::Preview, k).unwrap().exists());
                }
                validate_png(path)
            })
            .unwrap();
        assert_eq!(calls.get(), 3);
    }
    #[test]
    fn concurrent_stores_never_publish_torn_frames() {
        use std::sync::{Arc, Barrier};
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        let dest = s.frame_path(&cache_key(), Quality::Preview, 0).unwrap();
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, vec![1; 65536]).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for byte in [2, 3] {
                let root = &t.0;
                let barrier = Arc::clone(&barrier);
                handles.push(scope.spawn(move || {
                    std::panic::catch_unwind(|| {
                        let store = Store::new(root).unwrap();
                        barrier.wait();
                        for _ in 0..24 {
                            let chunk = store.begin_chunk(&cache_key(), Quality::Preview).unwrap();
                            write_frame(&chunk, 0, &vec![byte; 65536]);
                            chunk
                                .commit(0..1, |p| {
                                    assert_eq!(fs::read(p).unwrap(), vec![byte; 65536]);
                                    Ok(())
                                })
                                .unwrap();
                        }
                    })
                }));
            }
            barrier.wait();
            for _ in 0..128 {
                let bytes = fs::read(&dest).unwrap();
                assert_eq!(bytes.len(), 65536);
                assert!([1, 2, 3].contains(&bytes[0]));
                assert!(bytes.iter().all(|b| *b == bytes[0]));
            }
            for handle in handles {
                handle.join().unwrap().unwrap();
            }
        });
        assert!([2, 3].contains(&fs::read(dest).unwrap()[0]));
    }
    #[test]
    fn commit_propagates_destination_errors_and_removes_staging() {
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        let chunk = s.begin_chunk(&cache_key(), Quality::Preview).unwrap();
        let tmp = chunk.pattern().parent().unwrap().to_path_buf();
        write_frame(&chunk, 0, &png());
        fs::write(t.0.join("Scene Renders").join(cache_key()).join("preview"), b"sentinel").unwrap();
        assert!(chunk.commit(0..1, validate_png).is_err());
        assert!(!tmp.exists());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_frames_are_not_published_and_size_does_not_follow_links() {
        use std::os::unix::fs::symlink;
        let t = Temp::new();
        let s = Store::new(&t.0).unwrap();
        let external = t.0.join("external");
        fs::write(&external, png()).unwrap();
        let chunk = s.begin_chunk(&cache_key(), Quality::Preview).unwrap();
        symlink(&external, chunk.pattern().parent().unwrap().join("000000.png")).unwrap();
        assert!(chunk.commit(0..1, validate_png).is_err());
        symlink(&external, t.0.join("Scene Renders").join("external")).unwrap();
        assert_eq!(s.size(), 0);
        s.clear().unwrap();
        assert!(external.exists());
    }
}
