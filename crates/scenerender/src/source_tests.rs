use super::*;
use crate::fake::{Behaviour, FakeRenderer};
use filmcraft_media::{FrameRequest, MediaError, cancel};
use filmcraft_time::{FrameRate, Tick};
use std::sync::{Arc, Barrier, atomic::AtomicBool};
use std::time::Duration;

fn doc(audio: bool) -> Vec<u8> {
    format!(
        r#"<scene version="1.7"><project width="2" height="2" fps="24" duration="0.5"/>{}<composition/></scene>"#,
        if audio { r#"<assets><audio id="a" src="tone.wav"/></assets>"# } else { "" }
    )
    .into_bytes()
}
fn setup(fake: &FakeRenderer) -> (std::path::PathBuf, Env) {
    let path = fake.dir().join("document.scene");
    std::fs::write(&path, doc(false)).unwrap();
    (path, Env { cache_dir: Some(fake.dir().join("cache")), chunk_len: 4, timeout: Duration::from_secs(2), ..Env::default() })
}
fn opened(fake: &FakeRenderer, env: &Env, audio: bool) -> Opened {
    let path = fake.dir().join("document.scene");
    std::fs::write(&path, doc(audio)).unwrap();
    open_with(&path, &doc(audio), env, Some(fake.program().into()), Arc::new(process::ProcessRunner)).unwrap()
}
fn frame(s: &Opened, k: i64) -> filmcraft_media::Result<Arc<filmcraft_frame::VideoFrame>> {
    s.source.video_frame(FrameRequest::full(FrameRate::FPS_24.tick_of(k)))
}
fn calls(fake: &FakeRenderer, command: &str) -> Vec<String> {
    fake.invocations().into_iter().filter(|s| s.starts_with(command)).collect()
}

#[test]
fn source_info_identical_and_placeholder_cached() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (path, env) = setup(&fake);
    let real = opened(&fake, &env, true);
    let missing = open_with(&path, &doc(true), &env, None, Arc::new(process::ProcessRunner)).unwrap();
    assert_eq!(real.source.info(), missing.source.info());
    let info = real.source.info();
    assert_eq!(info.container, "sr-core scene");
    assert_eq!(info.file_size, Some(doc(true).len() as u64));
    assert_eq!(info.duration, Tick(filmcraft_time::TICKS_PER_SECOND / 2));
    assert_eq!((info.audio().unwrap().sample_rate, info.audio().unwrap().channels), (48_000, 2));
    assert!(missing.warning.as_ref().unwrap().contains("not found"));
    assert_eq!(missing.warning, missing.status.reason);
    let a = frame(&missing, 0).unwrap();
    let b = frame(&missing, 0).unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!((a.width, a.height), (2, 2));
    let silence = missing.source.audio(0, 17, 44_100).unwrap();
    assert_eq!((silence.channel_count(), silence.frames()), (2, 17));
    assert!(silence.channels.iter().flatten().all(|x| *x == 0.0));
}

#[test]
fn source_invalid_header_is_decode() {
    assert!(matches!(
        open_with(std::path::Path::new("/bad.scene"), b"bad", &Env::default(), None, Arc::new(process::ProcessRunner)),
        Err(MediaError::Decode(_))
    ));
}

#[test]
fn source_memory_disk_and_chunk_hits() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, env) = setup(&fake);
    let first = opened(&fake, &env, false);
    let a = frame(&first, 1).unwrap();
    assert_eq!((a.width, a.height), (2, 2));
    assert!(Arc::ptr_eq(&a, &frame(&first, 1).unwrap()));
    frame(&first, 3).unwrap();
    let second = opened(&fake, &env, false);
    frame(&second, 2).unwrap();
    let log = calls(&fake, "render");
    assert_eq!(log.len(), 1);
    assert!(log[0].contains("--frames 0..4"));
}

#[test]
fn source_quality_and_draft_have_separate_caches() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, mut env) = setup(&fake);
    let preview = opened(&fake, &env, false);
    frame(&preview, 0).unwrap();
    cancel::with_draft(true, || frame(&preview, 0)).unwrap();
    env.quality = Quality::Final;
    frame(&opened(&fake, &env, false), 0).unwrap();
    let log = calls(&fake, "render");
    assert_eq!(log.len(), 3);
    for q in ["preview", "draft", "final"] {
        assert!(log.iter().any(|s| s.contains(&format!("--quality {q}"))));
    }
}

#[test]
fn source_threads_render_chunk_once() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, env) = setup(&fake);
    let source = opened(&fake, &env, false).source;
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        for k in [1, 2] {
            let s = &source;
            let b = &barrier;
            scope.spawn(move || {
                b.wait();
                s.video_frame(FrameRequest::full(FrameRate::FPS_24.tick_of(k))).unwrap();
            });
        }
    });
    assert_eq!(calls(&fake, "render").len(), 1);
}

#[test]
fn source_failures_are_remembered_and_publish_nothing() {
    for behaviour in [Behaviour::ExitInvalid, Behaviour::ExitIo, Behaviour::WrongSize, Behaviour::NoOutput, Behaviour::Hang] {
        let fake = FakeRenderer::new(behaviour);
        let (_, mut env) = setup(&fake);
        env.timeout = Duration::from_millis(100);
        let source = opened(&fake, &env, false);
        assert!(frame(&source, 0).is_err(), "{behaviour:?}");
        assert!(frame(&source, 1).is_err(), "{behaviour:?}");
        assert_eq!(calls(&fake, "render").len(), 1, "{behaviour:?}");
        assert_eq!(store::Store::new(env.cache_dir.as_ref().unwrap()).unwrap().size(), 0);
    }
}

#[test]
fn source_cancellation_does_not_start_or_poison_chunk() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, env) = setup(&fake);
    let source = opened(&fake, &env, false);
    let flag = Arc::new(AtomicBool::new(true));
    assert!(matches!(cancel::with_cancel(&flag, || frame(&source, 0)), Err(MediaError::Cancelled)));
    assert!(calls(&fake, "render").is_empty());
    frame(&source, 0).unwrap();
}

#[test]
fn source_hostile_requests_one_frame_and_zero_chunk() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (path, mut env) = setup(&fake);
    env.chunk_len = 0;
    env.memory_budget = usize::MAX;
    let bytes = String::from_utf8(doc(false)).unwrap().replace("0.5", "0.01").into_bytes();
    let source = open_with(&path, &bytes, &env, Some(fake.program().into()), Arc::new(process::ProcessRunner)).unwrap();
    for time in [Tick(i64::MIN), Tick(i64::MAX)] {
        for scale in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.5] {
            assert_eq!(source.source.video_frame(FrameRequest { time, scale }).unwrap().width, 2);
        }
    }
    let log = calls(&fake, "render");
    assert_eq!(log.len(), 1);
    assert!(log[0].contains("--frames 0..1"));
}

#[test]
fn source_bad_cache_path_is_error_not_panic() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (path, mut env) = setup(&fake);
    env.cache_dir = Some(path);
    let source = opened(&fake, &env, false);
    assert!(frame(&source, 0).is_err());
    assert!(frame(&source, 0).is_err());
    assert!(calls(&fake, "render").is_empty());
}

#[test]
fn source_no_disk_cache_keeps_chunk_in_memory_and_cleans_temp() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, mut env) = setup(&fake);
    env.cache_dir = None;
    let source = opened(&fake, &env, false);
    frame(&source, 0).unwrap();
    frame(&source, 1).unwrap();
    let log = calls(&fake, "render");
    assert_eq!(log.len(), 1);
    let pattern = log[0].split(" -o ").nth(1).unwrap().split(" --bit-depth").next().unwrap();
    assert!(!std::path::Path::new(pattern).parent().unwrap().exists());
    frame(&opened(&fake, &env, false), 0).unwrap();
    assert_eq!(calls(&fake, "render").len(), 2);
}

#[test]
fn source_audio_streams_resamples_and_reuses_disk() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, env) = setup(&fake);
    std::fs::write(fake.dir().join("template.wav"), filmcraft_media::wav::write_wav16(&[0.0, 0.5, -0.5, 0.25], 1, 48_000)).unwrap();
    let source = opened(&fake, &env, true);
    let audio = source.source.audio(1, 3, 96_000).unwrap();
    assert_eq!((audio.channel_count(), audio.frames(), audio.sample_rate), (2, 3, 96_000));
    assert!((audio.channels[0][0] - 0.25).abs() < 0.001);
    assert!((audio.channels[0][1] - 0.5).abs() < 0.001);
    assert!(audio.channels[0][2].abs() < 0.001);
    assert_eq!(audio.channels[0], audio.channels[1]);
    source.source.audio(0, 8, 48_000).unwrap();
    opened(&fake, &env, true).source.audio(0, 8, 48_000).unwrap();
    assert_eq!(calls(&fake, "encode").len(), 1);
}

#[test]
fn source_audio_failure_silences_once() {
    for behaviour in [Behaviour::NoAudio, Behaviour::ExitIo, Behaviour::NoOutput, Behaviour::Hang] {
        let fake = FakeRenderer::new(behaviour);
        let (_, mut env) = setup(&fake);
        env.timeout = Duration::from_millis(100);
        let source = opened(&fake, &env, true);
        for _ in 0..2 {
            let audio = source.source.audio(0, 13, 44_100).unwrap();
            assert_eq!((audio.channel_count(), audio.frames()), (2, 13));
            assert!(audio.channels.iter().flatten().all(|v| *v == 0.0));
        }
        assert_eq!(calls(&fake, "encode").len(), 1);
    }
}

#[test]
fn source_audio_out_of_range_and_allocations_are_bounded() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, env) = setup(&fake);
    let source = opened(&fake, &env, true);
    for start in [-1, i64::MAX, 24_000] {
        let audio = source.source.audio(start, 9, 48_000).unwrap();
        assert_eq!(audio.frames(), 9);
        assert!(audio.channels.iter().flatten().all(|x| *x == 0.0));
    }
    assert!(calls(&fake, "encode").is_empty());
    let audio = source.source.audio(0, usize::MAX, 0).unwrap();
    assert!(audio.frames() <= 1_048_576);
}

#[test]
fn source_probe_is_cached_and_invalidated_by_file_size() {
    let fake = FakeRenderer::new(Behaviour::NoOutput);
    let config = discover::Config { program: Some(fake.program().into()) };
    for _ in 0..2 {
        let status = probe(&config);
        assert!(status.found, "{status:?}");
        assert_eq!(status.version.as_deref(), Some("0.5.0"));
        assert_eq!(status.schema.as_deref(), Some("1.7.0"));
    }
    assert_eq!(calls(&fake, "--version").len(), 1);
    use std::io::Write;
    std::fs::OpenOptions::new().append(true).open(fake.program()).unwrap().write_all(b"\n# changed\n").unwrap();
    assert!(probe(&config).found);
    assert_eq!(calls(&fake, "--version").len(), 2);
}

#[test]
fn source_old_or_failed_probe_falls_back_with_warning() {
    for (from, to) in [("0.5.0", "0.4.0"), ("scene-render 0.5.0", "garbage"), ("capabilities/1", "capabilities/2")] {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let (_, env) = setup(&fake);
        let script = std::fs::read_to_string(fake.program()).unwrap().replace(from, to);
        std::fs::write(fake.program(), script).unwrap();
        let source = opened(&fake, &env, false);
        assert!(!source.status.found);
        assert!(source.warning.is_some());
        frame(&source, 0).unwrap();
        assert!(calls(&fake, "render").is_empty());
    }
}

#[test]
fn source_real_renderer_oracle() {
    let Some(program) = std::env::var_os("SR_RENDER_BIN") else { return };
    let fake = FakeRenderer::new(Behaviour::NoOutput);
    let (path, mut env) = setup(&fake);
    let bytes = br##"<scene version="1.7"><project width="64" height="64" fps="24" duration="0.5" background="#000000FF"/><composition><shape id="r" shape="rect" width="40" height="40" x="32" y="32" fill="#FF3300FF"/></composition></scene>"##;
    std::fs::write(&path, bytes).unwrap();
    env.config.program = Some(program.into());
    env.timeout = Duration::from_secs(120);
    assert_eq!(header::parse(bytes).unwrap().frame_count(), 12);
    let source = open(&path, bytes, &env).unwrap();
    assert!(source.status.found, "{:?}", source.warning);
    let image = frame(&source, 0).unwrap();
    assert_eq!((image.width, image.height), (64, 64));
    assert!(image.to_rgba8().as_chunks::<4>().0.iter().any(|p| p[0] > 100));
    let before = store::Store::new(env.cache_dir.as_ref().unwrap()).unwrap().size();
    let second = open_with(&path, bytes, &env, env.config.program.clone(), Arc::new(ProbeOnly)).unwrap();
    assert_eq!(frame(&second, 0).unwrap().to_rgba8(), image.to_rgba8());
    assert_eq!(store::Store::new(env.cache_dir.as_ref().unwrap()).unwrap().size(), before);
}

struct ProbeOnly;
impl process::Runner for ProbeOnly {
    fn run(
        &self,
        program: &std::path::Path,
        args: &[std::ffi::OsString],
        cwd: &std::path::Path,
        timeout: Duration,
    ) -> filmcraft_media::Result<process::RunOutput> {
        assert!(args.first().is_some_and(|s| s == "--version" || s == "capabilities"), "disk cache missed: {args:?}");
        process::Runner::run(&process::ProcessRunner, program, args, cwd, timeout)
    }
}

#[test]
fn source_draft_accepts_renderer_half_size_and_restores_header_size() {
    let fake = FakeRenderer::new(Behaviour::WrongSize);
    let (_, env) = setup(&fake);
    let source = opened(&fake, &env, false);
    let image = cancel::with_draft(true, || frame(&source, 2)).unwrap();
    assert_eq!((image.width, image.height), (2, 2));
    assert_eq!(image.pts, FrameRate::FPS_24.tick_of(2));
    assert!(image.to_rgba8().as_chunks::<4>().0.iter().all(|pixel| *pixel == [200, 100, 50, 128]));
    assert!(frame(&source, 2).is_err());
}

#[test]
fn source_native_program_paths_are_lossless() {
    use std::os::unix::ffi::OsStringExt;
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (path, mut env) = setup(&fake);
    let program = fake.dir().join(std::ffi::OsString::from_vec(b"render-\xff".to_vec()));
    std::fs::copy(fake.program(), &program).unwrap();
    let injected = open_with(&path, &doc(false), &env, Some(program.clone()), Arc::new(process::ProcessRunner)).unwrap();
    assert!(injected.status.found, "{:?}", injected.warning);
    frame(&injected, 0).unwrap();
    env.config.program = Some(program);
    let system = open(&path, &doc(false), &env).unwrap();
    assert!(system.status.found, "{:?}", system.warning);
    frame(&system, 4).unwrap();
    assert_eq!(calls(&fake, "render").len(), 2);
}

#[test]
fn source_invalid_wav_signature_is_not_published() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, env) = setup(&fake);
    let mut wav = filmcraft_media::wav::write_wav16(&[0.5; 48], 1, 48_000);
    wav[..4].copy_from_slice(b"NOPE");
    std::fs::write(fake.dir().join("template.wav"), wav).unwrap();
    let source = opened(&fake, &env, true);
    let audio = source.source.audio(0, 13, 48_000).unwrap();
    assert!(audio.channels.iter().flatten().all(|v| *v == 0.0));
    assert_eq!(store::Store::new(env.cache_dir.as_ref().unwrap()).unwrap().size(), 0);
}

#[test]
fn source_trims_missing_disk_span() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, env) = setup(&fake);
    frame(&opened(&fake, &env, false), 0).unwrap();
    let cache = store::Store::new(env.cache_dir.as_ref().unwrap()).unwrap();
    let key = store::key("0.5.0", Quality::Preview, &doc(false), &[]);
    for k in [1, 2] {
        std::fs::remove_file(cache.frame_path(&key, Quality::Preview, k).unwrap()).unwrap();
    }
    frame(&opened(&fake, &env, false), 1).unwrap();
    let log = calls(&fake, "render");
    assert_eq!(log.len(), 2);
    assert!(log[1].contains("--frames 1..3"));
}

#[test]
fn source_audio_without_cache_keeps_streaming_file_until_drop() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, mut env) = setup(&fake);
    env.cache_dir = None;
    std::fs::write(fake.dir().join("template.wav"), filmcraft_media::wav::write_wav16(&[0.5; 48], 1, 48_000)).unwrap();
    let source = opened(&fake, &env, true);
    for _ in 0..2 {
        assert!(source.source.audio(0, 8, 48_000).unwrap().channels[0][0] > 0.4);
    }
    let log = calls(&fake, "encode");
    assert_eq!(log.len(), 1);
    let staged = std::path::Path::new(log[0].split(" -o ").nth(1).unwrap());
    let parent = staged.parent().unwrap().parent().unwrap();
    assert!(parent.join("mix.wav").is_file());
    let temp = parent.parent().unwrap().parent().unwrap().to_owned();
    drop(source);
    assert!(!temp.exists());
}

#[test]
fn source_header_without_audio_encodes_once_then_silences() {
    let fake = FakeRenderer::new(Behaviour::NoAudio);
    let (_, env) = setup(&fake);
    let source = opened(&fake, &env, false);
    assert!(source.source.info().audio_streams.is_empty());
    for _ in 0..2 {
        assert!(source.source.audio(0, 8, 48_000).unwrap().channels.iter().flatten().all(|v| *v == 0.0));
    }
    assert_eq!(calls(&fake, "encode").len(), 1);
}

#[test]
fn source_cancel_running_renderer_cleans_and_can_retry() {
    use std::sync::atomic::Ordering;
    let fake = FakeRenderer::new(Behaviour::Hang);
    let (_, env) = setup(&fake);
    let source = opened(&fake, &env, false);
    let flag = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| cancel::with_cancel(&flag, || frame(&source, 0)));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while calls(&fake, "render").is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        flag.store(true, Ordering::Relaxed);
        assert!(matches!(worker.join().unwrap(), Err(MediaError::Cancelled)));
    });
    assert_eq!(store::Store::new(env.cache_dir.as_ref().unwrap()).unwrap().size(), 0);
    let script = std::fs::read_to_string(fake.program()).unwrap().replace("mode='hang'", "mode='ok'");
    std::fs::write(fake.program(), script).unwrap();
    frame(&source, 0).unwrap();
    assert_eq!(calls(&fake, "render").len(), 2);
}

#[test]
fn source_process_limit_is_shared_across_sources() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Counting {
        active: AtomicUsize,
        peak: AtomicUsize,
    }
    impl process::Runner for Counting {
        fn run(
            &self,
            program: &std::path::Path,
            args: &[std::ffi::OsString],
            cwd: &std::path::Path,
            timeout: Duration,
        ) -> filmcraft_media::Result<process::RunOutput> {
            let n = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            let result = process::Runner::run(&process::ProcessRunner, program, args, cwd, timeout);
            self.active.fetch_sub(1, Ordering::SeqCst);
            result
        }
    }
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (path, mut env) = setup(&fake);
    env.cache_dir = None;
    let runner = Arc::new(Counting { active: AtomicUsize::new(0), peak: AtomicUsize::new(0) });
    let a = open_with(&path, &doc(false), &env, Some(fake.program().into()), runner.clone()).unwrap();
    let b = open_with(&path, &doc(false), &env, Some(fake.program().into()), runner.clone()).unwrap();
    let barrier = Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            barrier.wait();
            frame(&a, 0).unwrap();
        });
        scope.spawn(|| {
            barrier.wait();
            frame(&b, 0).unwrap();
        });
    });
    assert_eq!(runner.peak.load(Ordering::SeqCst), 1);
    assert_eq!(calls(&fake, "render").len(), 2);
}

#[test]
fn source_audio_downmixes_surround_keeping_centre() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, env) = setup(&fake);
    // 5.1 (L R C LFE Ls Rs), dialogue on the centre channel only, then left surround only
    let samples = [0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0];
    std::fs::write(fake.dir().join("template.wav"), filmcraft_media::wav::write_wav16(&samples, 6, 48_000)).unwrap();
    let source = opened(&fake, &env, true);
    let audio = source.source.audio(0, 2, 48_000).unwrap();
    assert_eq!(audio.channel_count(), 2);
    assert!((audio.channels[0][0] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "{:?}", audio.channels);
    assert!((audio.channels[1][0] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "{:?}", audio.channels);
    assert!(audio.channels[0][1] > 0.3 && audio.channels[1][1].abs() < 0.01, "{:?}", audio.channels);
}

#[test]
fn source_cancelled_mixdown_is_retried() {
    use std::sync::atomic::Ordering;
    let fake = FakeRenderer::new(Behaviour::Hang);
    let (_, env) = setup(&fake);
    std::fs::write(fake.dir().join("template.wav"), filmcraft_media::wav::write_wav16(&[0.5, 0.5], 1, 48_000)).unwrap();
    let source = opened(&fake, &env, true);
    let flag = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| cancel::with_cancel(&flag, || source.source.audio(0, 2, 48_000)));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while calls(&fake, "encode").is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        flag.store(true, Ordering::Relaxed);
        assert!(worker.join().unwrap().unwrap().channels.iter().flatten().all(|v| *v == 0.0));
    });
    let script = std::fs::read_to_string(fake.program()).unwrap().replace("mode='hang'", "mode='ok'");
    std::fs::write(fake.program(), script).unwrap();
    assert!(source.source.audio(0, 2, 48_000).unwrap().channels[0][0] > 0.4);
    assert_eq!(calls(&fake, "encode").len(), 2);
}

#[test]
fn source_audio_is_not_blocked_by_a_video_render() {
    let fake = FakeRenderer::new(Behaviour::Ok { width: 2, height: 2 });
    let (_, mut env) = setup(&fake);
    env.timeout = Duration::from_secs(4);
    std::fs::write(fake.dir().join("template.wav"), filmcraft_media::wav::write_wav16(&[0.5, 0.5], 1, 48_000)).unwrap();
    let source = opened(&fake, &env, true);
    assert!(source.source.audio(0, 2, 48_000).unwrap().channels[0][0] > 0.4);
    let script = std::fs::read_to_string(fake.program()).unwrap().replace("mode='ok'", "mode='hang'");
    std::fs::write(fake.program(), script).unwrap();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| frame(&source, 0));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while calls(&fake, "render").is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let started = std::time::Instant::now();
        assert!(source.source.audio(0, 2, 48_000).unwrap().channels[0][0] > 0.4);
        assert!(started.elapsed() < Duration::from_secs(1), "audio waited {:?} for the video render", started.elapsed());
        assert!(worker.join().unwrap().is_err());
    });
}
