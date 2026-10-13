//! Scene media providers with header-only metadata and bounded frame/audio caches.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use crate::header::SceneHeader;
use filmcraft_frame::{AudioBuffer, VideoFrame};
use filmcraft_media::{AudioStreamInfo, FrameRequest, MediaError, MediaInfo, MediaKind, MediaSource, Result, VideoStreamInfo, cancel};
use std::{
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

#[cfg(not(target_arch = "wasm32"))]
const MAX_FRAME_BYTES: usize = 256 << 20;
const MAX_AUDIO_FRAMES: usize = 1 << 20;
const AUDIO_CHANNELS: usize = 2;

pub(crate) fn media_info(path: &Path, file_size: usize, header: &SceneHeader) -> MediaInfo {
    MediaInfo {
        name: path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned(),
        kind: MediaKind::Movie,
        duration: header.duration,
        video: Some(VideoStreamInfo {
            width: header.width,
            height: header.height,
            frame_rate: header.fps,
            par: (1, 1),
            codec: "scene-render".into(),
            pixel_format: "RGBA 8-bit".into(),
            color: VideoFrame::rgba8(0, 0, Vec::new()).color,
            has_alpha: true,
            bitrate: None,
            hdr: None,
        }),
        audio_streams: if header.has_audio {
            vec![AudioStreamInfo { sample_rate: 48_000, channels: AUDIO_CHANNELS as u32, codec: "PCM".into(), bits_per_sample: Some(24) }]
        } else {
            Vec::new()
        },
        container: "sr-core scene".into(),
        start_timecode: None,
        file_size: Some(file_size as u64),
    }
}

/// A scene without a usable renderer; its metadata remains available.
pub struct PlaceholderSource {
    info: MediaInfo,
    header: SceneHeader,
    frame: Mutex<Option<Arc<VideoFrame>>>,
}
impl PlaceholderSource {
    pub(crate) fn new(info: MediaInfo, header: SceneHeader) -> Self {
        Self { info, header, frame: Mutex::new(None) }
    }
}
impl MediaSource for PlaceholderSource {
    fn info(&self) -> &MediaInfo {
        &self.info
    }
    fn video_frame(&self, _req: FrameRequest) -> Result<Arc<VideoFrame>> {
        if cancel::cancelled() {
            return Err(MediaError::Cancelled);
        }
        let mut cached = self.frame.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(frame) = &*cached {
            return Ok(frame.clone());
        }
        let frame = Arc::new(crate::placeholder::slate(self.header.width, self.header.height)?);
        *cached = Some(frame.clone());
        Ok(frame)
    }
    fn audio(&self, _start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer> {
        Ok(silence(frames, sample_rate))
    }
}

fn silence(frames: usize, sample_rate: u32) -> AudioBuffer {
    AudioBuffer::silence(sample_rate, AUDIO_CHANNELS, frames.min(MAX_AUDIO_FRAMES))
}

/// Folds the mixdown's channels into stereo. Mono feeds both sides; layouts wider than stereo follow the
/// WAV order L, R, C, LFE, then surround pairs: centre and surrounds join at -3 dB, LFE is dropped.
/// Ambisonic mixes are not decoded and fold by the same rule (a documented approximation).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn downmix(src: &[Vec<f32>], dst: &mut [Vec<f32>]) {
    const GAIN: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let finite = |value: f32| if value.is_finite() { value } else { 0.0 };
    for (c, dst) in dst.iter_mut().enumerate() {
        let side = c % 2;
        for (i, out) in dst.iter_mut().enumerate() {
            let sample = |channel: usize| src.get(channel).and_then(|samples| samples.get(i)).copied().map_or(0.0, finite);
            *out = match src.len() {
                0 => 0.0,
                1 => sample(0),
                n => {
                    let mut value = sample(side);
                    if n > 2 {
                        value += GAIN * sample(2);
                    }
                    let mut channel = 4 + side;
                    while channel < n {
                        value += GAIN * sample(channel);
                        channel += 2;
                    }
                    value
                }
            };
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::SceneRenderSource;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::process_lock;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use crate::{
        Env, Quality, args, ingest, process,
        store::{self, Store},
        timing,
    };
    use filmcraft_media::{ByteReader, cache::FrameCache, reader::FileReader, wav::WavSource};
    use std::{
        collections::HashMap,
        fs,
        path::PathBuf,
        sync::{
            MutexGuard, TryLockError,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    static PROCESS: Mutex<()> = Mutex::new(());

    pub(crate) fn process_lock() -> Result<MutexGuard<'static, ()>> {
        loop {
            if cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            match PROCESS.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Result<Self> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            for _ in 0..64 {
                let path = std::env::temp_dir().join(format!("filmcraft-scene-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(io_error(error)),
                }
            }
            Err(MediaError::Io("cannot allocate a unique scene-render temporary directory".into()))
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    enum AudioState {
        Pending,
        Ready { wav: Box<WavSource>, _temp: Option<TempDir> },
        Failed,
    }
    struct State {
        failed: HashMap<(Quality, u64), String>,
        exhausted: Option<String>,
    }

    /// A renderer-backed scene. Construct through [`crate::open`] or [`crate::open_with`].
    pub struct SceneRenderSource {
        info: MediaInfo,
        header: SceneHeader,
        path: PathBuf,
        program: PathBuf,
        runner: Arc<dyn process::Runner>,
        env: Env,
        keys: [(Quality, String); 3],
        frames: FrameCache<(Quality, u64)>,
        state: Mutex<State>,
        /// Separate from `state` so audio never waits behind a video render's state lock.
        audio: Mutex<AudioState>,
    }

    impl SceneRenderSource {
        #[allow(clippy::too_many_arguments)]
        pub(crate) fn new(
            path: &Path,
            bytes: &[u8],
            env: &Env,
            header: SceneHeader,
            info: MediaInfo,
            program: PathBuf,
            version: &str,
            runner: Arc<dyn process::Runner>,
        ) -> Self {
            let stamps = store::stamp_assets(path.parent().unwrap_or(path), &header.asset_refs);
            let keys = [Quality::Draft, Quality::Preview, Quality::Final].map(|quality| (quality, store::key(version, quality, bytes, &stamps)));
            let mut env = env.clone();
            env.chunk_len = env.chunk_len.clamp(1, 65_536);
            env.memory_budget = env.memory_budget.min(MAX_FRAME_BYTES);
            env.timeout = env.timeout.min(Duration::from_secs(3600));
            Self {
                info,
                header,
                path: path.into(),
                program,
                runner,
                frames: FrameCache::new(env.memory_budget),
                env,
                keys,
                state: Mutex::new(State { failed: HashMap::new(), exhausted: None }),
                audio: Mutex::new(AudioState::Pending),
            }
        }
        fn key(&self, quality: Quality) -> Result<&str> {
            self.keys.iter().find(|(q, _)| *q == quality).map(|(_, key)| key.as_str()).ok_or_else(|| MediaError::Unsupported("unknown scene quality".into()))
        }
        fn read(&self, path: &Path, quality: Quality, k: u64) -> Result<Arc<VideoFrame>> {
            let frame = self.read_pixels(path, quality)?;
            let pts = timing::frame_time(k, self.header.fps).ok_or_else(|| MediaError::Decode("scene frame time exceeds Tick range".into()))?;
            Ok(Arc::new(frame.with_pts(pts)))
        }
        fn read_pixels(&self, path: &Path, quality: Quality) -> Result<VideoFrame> {
            let (width, height) = (self.header.width, self.header.height);
            match ingest::read_frame(path, width, height, MAX_FRAME_BYTES as u64) {
                Ok(frame) => Ok(frame),
                Err(error) if quality == Quality::Draft => {
                    // `render` writes draft at half size, rounding odd dimensions up; `encode` upscales it.
                    let small = ingest::read_frame(path, width.div_ceil(2), height.div_ceil(2), MAX_FRAME_BYTES as u64).map_err(|_| error)?;
                    let len = u64::from(width)
                        .checked_mul(u64::from(height))
                        .and_then(|n| n.checked_mul(4))
                        .filter(|n| *n <= MAX_FRAME_BYTES as u64)
                        .and_then(|n| usize::try_from(n).ok())
                        .ok_or_else(|| MediaError::Decode("scene draft exceeds pixel budget".into()))?;
                    let mut pixels = Vec::new();
                    pixels.try_reserve_exact(len).map_err(io_error)?;
                    let filmcraft_frame::PixelData::Rgba8(data) = &small.data else {
                        return Err(MediaError::Decode("scene draft is not RGBA8".into()));
                    };
                    for y in 0..height {
                        for x in 0..width {
                            let sx = u64::from(x) * u64::from(small.width) / u64::from(width);
                            let sy = u64::from(y) * u64::from(small.height) / u64::from(height);
                            let offset = usize::try_from((sy * u64::from(small.width) + sx) * 4).map_err(io_error)?;
                            let pixel = data.get(offset..offset.saturating_add(4)).ok_or_else(|| MediaError::Decode("scene draft pixel missing".into()))?;
                            pixels.extend_from_slice(pixel);
                        }
                    }
                    Ok(VideoFrame::rgba8(width, height, pixels))
                }
                Err(error) => Err(error),
            }
        }
        fn run(&self, args: &[std::ffi::OsString]) -> Result<()> {
            if cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            let cwd = self.path.parent().ok_or_else(|| MediaError::Unsupported("scene has no asset base directory".into()))?;
            let out = self.runner.run(&self.program, args, cwd, self.env.timeout)?;
            process::check(&out)?;
            if cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            Ok(())
        }
        fn video(&self, k: u64, quality: Quality) -> Result<Arc<VideoFrame>> {
            let temporary = if self.env.cache_dir.is_none() { Some(TempDir::new()?) } else { None };
            let root = self
                .env
                .cache_dir
                .as_deref()
                .or_else(|| temporary.as_ref().map(|t| t.0.as_path()))
                .ok_or_else(|| MediaError::Io("scene cache directory unavailable".into()))?;
            let store = Store::new(root)?;
            let key = self.key(quality)?;
            if store.has_frame(key, quality, k)? {
                return self.read(&store.frame_path(key, quality, k)?, quality, k);
            }
            let range = timing::chunk(k, self.env.chunk_len, self.header.frame_count());
            let mut missing = None;
            for index in range.clone() {
                if !store.has_frame(key, quality, index)? {
                    let span = missing.get_or_insert(index..index.saturating_add(1));
                    span.end = index.saturating_add(1);
                }
            }
            if let Some(span) = missing {
                let staging = store.begin_chunk(key, quality)?;
                self.run(&args::render_args(&self.path, span.clone(), &staging.pattern(), quality)?)?;
                staging.commit(span, |path| {
                    if cancel::cancelled() {
                        return Err(MediaError::Cancelled);
                    }
                    self.read_pixels(path, quality).map(|_| ())
                })?;
            }
            if temporary.is_some() {
                for index in range.filter(|i| *i != k) {
                    let frame = self.read(&store.frame_path(key, quality, index)?, quality, index)?;
                    self.frames.insert((quality, index), frame);
                }
            }
            self.read(&store.frame_path(key, quality, k)?, quality, k)
        }

        fn open_wav(path: &Path) -> Result<WavSource> {
            if !fs::symlink_metadata(path).map_err(io_error)?.is_file() {
                return Err(MediaError::Decode("scene mix must be a regular WAV file".into()));
            }
            let reader = FileReader::open(path).map_err(io_error)?;
            let mut signature = [0u8; 12];
            reader.read_at(0, &mut signature).map_err(io_error)?;
            if !filmcraft_media::wav::sniff(&signature) {
                return Err(MediaError::Decode("scene mix has an invalid WAV signature".into()));
            }
            let wav = WavSource::open("scene mix", Arc::new(reader))?;
            let info = wav.info().audio().ok_or(MediaError::NoStream("audio"))?;
            if info.sample_rate == 0 || info.sample_rate > 768_000 || info.channels == 0 || info.channels > 64 {
                return Err(MediaError::Decode("scene mix has unsupported rate or channel count".into()));
            }
            Ok(wav)
        }
        fn mixdown(&self) -> Result<AudioState> {
            let _global = process_lock()?;
            let temporary = if self.env.cache_dir.is_none() { Some(TempDir::new()?) } else { None };
            let root = self
                .env
                .cache_dir
                .as_deref()
                .or_else(|| temporary.as_ref().map(|t| t.0.as_path()))
                .ok_or_else(|| MediaError::Io("scene audio cache directory unavailable".into()))?;
            let store = Store::new(root)?;
            let key = self.key(Quality::Final)?;
            let anchor = store.frame_path(key, Quality::Final, 0)?;
            let destination = anchor.parent().and_then(Path::parent).ok_or_else(|| MediaError::Io("invalid scene audio cache path".into()))?.join("mix.wav");
            if destination.try_exists().map_err(io_error)? {
                return Ok(AudioState::Ready { wav: Box::new(Self::open_wav(&destination)?), _temp: temporary });
            }
            let staging = store.begin_chunk(key, Quality::Final)?;
            let pattern = staging.pattern();
            let wav_path = pattern.parent().ok_or_else(|| MediaError::Io("invalid scene audio staging path".into()))?.join("mix.wav");
            self.run(&args::audio_args(&self.path, &wav_path)?)?;
            // Close the validator before rename, which also works on Windows.
            drop(Self::open_wav(&wav_path)?);
            if cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            fs::rename(&wav_path, &destination).map_err(io_error)?;
            let wav = Self::open_wav(&destination)?;
            Ok(AudioState::Ready { wav: Box::new(wav), _temp: temporary })
        }
    }

    fn io_error(error: impl std::fmt::Display) -> MediaError {
        MediaError::Io(format!("scene-render cache: {error}"))
    }

    impl MediaSource for SceneRenderSource {
        fn info(&self) -> &MediaInfo {
            &self.info
        }
        fn video_frame(&self, req: FrameRequest) -> Result<Arc<VideoFrame>> {
            if cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            let count = self.header.frame_count();
            if count == 0 {
                return Err(MediaError::Decode("scene contains no frames".into()));
            }
            let k = timing::frame_at(req.time, self.header.fps, count);
            let quality = if cancel::draft() { Quality::Draft } else { self.env.quality };
            if let Some(frame) = self.frames.get(&(quality, k)) {
                return Ok(frame);
            }
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if cancel::cancelled() {
                return Err(MediaError::Cancelled);
            }
            if let Some(frame) = self.frames.get(&(quality, k)) {
                return Ok(frame);
            }
            let chunk = timing::chunk(k, self.env.chunk_len, count).start;
            if let Some(reason) = state.failed.get(&(quality, chunk)).or(state.exhausted.as_ref()) {
                return Err(MediaError::Decode(reason.clone()));
            }
            let _global = process_lock()?;
            match self.video(k, quality) {
                Ok(frame) => {
                    self.frames.insert((quality, k), frame.clone());
                    Ok(frame)
                }
                Err(MediaError::Cancelled) => Err(MediaError::Cancelled),
                Err(error) => {
                    let reason = error.to_string();
                    if state.failed.len() < 4096 {
                        state.failed.insert((quality, chunk), reason);
                    } else {
                        state.exhausted = Some(reason);
                    }
                    Err(error)
                }
            }
        }
        fn audio(&self, start: i64, frames: usize, sample_rate: u32) -> Result<AudioBuffer> {
            let frames = frames.min(MAX_AUDIO_FRAMES);
            let mut output = silence(frames, sample_rate);
            let total = (i128::from(self.header.duration.0) * i128::from(sample_rate)) / i128::from(filmcraft_time::TICKS_PER_SECOND);
            if frames == 0 || sample_rate == 0 || start < 0 || i128::from(start) >= total || cancel::cancelled() {
                return Ok(output);
            }
            let valid = usize::try_from(total.saturating_sub(i128::from(start))).unwrap_or(usize::MAX).min(frames);
            let mut audio = self.audio.lock().unwrap_or_else(PoisonError::into_inner);
            if matches!(*audio, AudioState::Pending) {
                match self.mixdown() {
                    Ok(ready) => *audio = ready,
                    // A cancelled mixdown is retried by the next request.
                    Err(MediaError::Cancelled) => return Ok(output),
                    Err(error) => {
                        log::warn!("scene audio {}: {error}; using silence", self.path.display());
                        *audio = AudioState::Failed;
                    }
                }
            }
            if let AudioState::Ready { wav, .. } = &*audio {
                match wav.audio(start, valid, sample_rate) {
                    Ok(mix) => downmix(&mix.channels, &mut output.channels),
                    Err(error) => {
                        log::warn!("scene audio {}: {error}; using silence", self.path.display());
                        *audio = AudioState::Failed;
                    }
                }
            }
            Ok(output)
        }
    }
}
