use super::*;
use crate::common::{Builder, MediaSpec};
use crate::scene::{Limits, read, validate, values};
use filmcraft_geom::Vec2;
use filmcraft_project::{
    Caption, CaptionTrack, ClipId, EffectInstance, Interpolation, ItemKind, Keyframe, Label, Marker, MarkerId, MarkerKind, Param, ParamValue, SequenceSettings,
    TrackId, TrackKind, Transition, TransitionAlign, TransitionId, graphic,
};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND};
use proptest::prelude::*;

fn fx(id: &str) -> EffectInstance {
    filmcraft_project::find_effect(id).unwrap().instance()
}
fn t(s: i64) -> Tick {
    Tick(s * TICKS_PER_SECOND)
}
fn fixture() -> (Builder, ItemId) {
    let mut b = Builder::new("Scene");
    let id = b.reserve_sequence("Sequence", SequenceSettings { width: 1920, height: 1080, frame_rate: FrameRate::FPS_24, ..Default::default() }, None);
    let media = b.file_media(
        "movie",
        "Movie",
        "/media/a.mov",
        &MediaSpec { duration: Some(t(30)), video: Some((640, 360, FrameRate::FPS_24)), audio: Some((48000, 2)), ..Default::default() },
        None,
    );
    let mut track = b.track(TrackKind::Video, 0);
    track.items.push(b.clip(media, TrackKind::Video, "First", t(0), t(4), t(0)));
    b.p.sequence_mut(id).unwrap().video_tracks.push(track);
    (b, id)
}
fn output(b: &Builder, id: ItemId) -> (String, Report) {
    let mut report = Report::default();
    let xml = export(&b.p, id, &ExportOptions::default(), &mut report).unwrap();
    checked(&xml);
    (xml, report)
}
fn checked(xml: &str) {
    let doc = read::read(xml, &Limits::default()).unwrap();
    let errors: Vec<_> = validate::validate(&doc.doc).into_iter().filter(|d| d.is_error()).collect();
    assert!(errors.is_empty(), "{errors:?}\n{xml}");
}
fn attr(xml: &str, tag: &str, name: &str) -> String {
    let doc = read::read(xml, &Limits::default()).unwrap();
    doc.root().descendants().find(|n| n.has_tag_name(tag)).unwrap().attribute(name).unwrap().to_owned()
}
fn gate(xml: &str, rule: &str) {
    assert_eq!(attr(xml, "scene", "version"), "1.1");
    let lower = xml.replacen("version=\"1.1\"", "version=\"1.0\"", 1);
    let doc = read::read(&lower, &Limits::default()).unwrap();
    assert!(validate::validate(&doc.doc).iter().any(|d| d.is_error() && d.code == rule));
}
#[test]
fn scene_export_one_clip() {
    let (b, id) = fixture();
    let (xml, report) = output(&b, id);
    // Four timeline seconds, untrimmed; 640x360 source centred in 1920x1080.
    assert_eq!(attr(&xml, "scene", "version"), "1.0");
    for (name, value) in [("start", "0"), ("end", "4"), ("clipIn", "0"), ("clipOut", "4"), ("x", "960"), ("y", "540"), ("anchorX", "320"), ("anchorY", "180")] {
        assert_eq!(attr(&xml, "layer", name), value);
    }
    assert_eq!(attr(&xml, "project", "duration"), "4");
    assert!(!report.has_warnings(), "{report}");
}
#[test]
fn scene_export_trim_speed_and_reverse() {
    let (mut b, id) = fixture();
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.start = t(2);
    c.duration = t(3);
    c.source_in = t(5);
    c.speed = 2.0;
    c.reverse = true;
    let (xml, _) = output(&b, id);
    // Source span is 3s * 2 = 6s, starting at 5s: [5,11), timeline [2,5).
    for (name, value) in [("start", "2"), ("end", "5"), ("clipIn", "5"), ("clipOut", "11"), ("speed", "2"), ("reverse", "true")] {
        assert_eq!(attr(&xml, "layer", name), value);
    }
}
#[test]
fn scene_export_track_paint_order_and_dedup() {
    let (mut b, id) = fixture();
    let mut upper = b.p.sequence(id).unwrap().video_tracks[0].clone();
    upper.id = TrackId(100);
    upper.items[0].id = ClipId(101);
    upper.items[0].name = "Upper".into();
    b.p.sequence_mut(id).unwrap().video_tracks.push(upper);
    let (xml, _) = output(&b, id);
    let doc = read::read(&xml, &Limits::default()).unwrap();
    let layers: Vec<_> = doc.root().descendants().filter(|n| n.has_tag_name("layer")).collect();
    assert_eq!(layers.iter().map(|n| n.attribute("name").unwrap()).collect::<Vec<_>>(), ["First", "Upper"]);
    assert_eq!(doc.root().descendants().filter(|n| n.has_tag_name("video")).count(), 1);
}
#[test]
fn scene_export_motion_anchor() {
    let (mut b, id) = fixture();
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.scale_to_frame = true;
    let m = c.effect_mut("motion").unwrap();
    m.params.insert("position".into(), Param::new(ParamValue::Vec2(Vec2::new(100.0, 200.0))));
    m.params.insert("anchor".into(), Param::new(ParamValue::Vec2(Vec2::new(10.0, 20.0))));
    m.params.insert("scale".into(), Param::new(ParamValue::Float(200.0)));
    m.params.insert("rotation".into(), Param::new(ParamValue::Float(90.0)));
    let (xml, _) = output(&b, id);
    // Natural source coordinates retain anchor (10,20); fitting is folded into scale:
    // min(1920/640,1080/360) * 200% = 6. T(100,200) R(90) S(6) T(-10,-20).
    for (name, value) in [("x", "100"), ("y", "200"), ("anchorX", "10"), ("anchorY", "20"), ("scaleX", "6"), ("scaleY", "6"), ("rotation", "90")] {
        assert_eq!(attr(&xml, "layer", name), value);
    }
}
#[test]
fn scene_export_opacity_keys_outgoing_ease() {
    let (mut b, id) = fixture();
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.start = t(5);
    let p = c.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap();
    p.keyframes = vec![Keyframe::new(t(0), ParamValue::Float(0.0)), Keyframe::new(t(2), ParamValue::Float(100.0))];
    p.keyframes[0].interp = Interpolation::EaseOut;
    let (xml, _) = output(&b, id);
    assert_eq!(attr(&xml, "animate", "timeBase"), "local");
    assert_eq!(attr(&xml, "animate", "property"), "opacity");
    assert_eq!(attr(&xml, "key", "time"), "0");
    assert_eq!(attr(&xml, "key", "value"), "0");
    // FilmCraft EaseOut flattens the departing handle; CSS ease-out is a different curve.
    assert_eq!(attr(&xml, "key", "interpolation"), "cubic-bezier");
    assert_eq!(attr(&xml, "key", "bezier"), "0.3333333333333333,0,0.6666666666666666,0.6666666666666666");
    assert_eq!(attr(&xml, "scene", "version"), "1.0");
}
#[test]
fn scene_export_transition_gate() {
    let (mut b, id) = fixture();
    let seq = b.p.sequence_mut(id).unwrap();
    let track = &mut seq.video_tracks[0];
    let mut next = track.items[0].clone();
    next.id = ClipId(50);
    next.start = t(4);
    track.items.push(next);
    track.transitions.push(Transition {
        id: TransitionId(51),
        effect: fx("cross_dissolve"),
        start: t(3),
        duration: t(2),
        from: Some(track.items[0].id),
        to: Some(ClipId(50)),
        align: TransitionAlign::CenterAtCut,
        reverse: false,
    });
    let (xml, _) = output(&b, id);
    gate(&xml, "V3");
    assert_eq!(attr(&xml, "transition", "type"), "crossfade");
    assert_eq!(attr(&xml, "transition", "duration"), "2");
    assert_eq!(attr(&xml, "transition", "alignment"), "center");
    assert_eq!(attr(&xml, "transition", "curve"), "linear");
}
#[test]
fn scene_export_markers_gate() {
    let (mut b, id) = fixture();
    b.p.sequence_mut(id).unwrap().markers.push(Marker {
        id: MarkerId(70),
        start: t(1),
        duration: t(2),
        name: "Chapter".into(),
        comment: String::new(),
        kind: MarkerKind::Chapter,
        color: Label::Blue,
    });
    let (xml, _) = output(&b, id);
    gate(&xml, "V1");
    assert_eq!(attr(&xml, "marker", "time"), "1");
    assert_eq!(attr(&xml, "marker", "duration"), "2");
    assert_eq!(attr(&xml, "marker", "kind"), "chapter");
}
#[test]
fn scene_export_captions_gate() {
    let (mut b, id) = fixture();
    let mut track = CaptionTrack::new(TrackId(80), "Captions".into(), Default::default());
    track.language = "en".into();
    track.captions.push(Caption {
        id: ClipId(81),
        start: t(1),
        duration: t(2),
        text: "Hello & goodbye".into(),
        speaker: None,
        cue_id: None,
        settings: String::new(),
    });
    b.p.sequence_mut(id).unwrap().caption_tracks.push(track);
    let (xml, _) = output(&b, id);
    gate(&xml, "V1");
    assert_eq!(attr(&xml, "cue", "start"), "1");
    assert_eq!(attr(&xml, "cue", "end"), "3");
    assert_eq!(attr(&xml, "cue", "text"), "Hello & goodbye");
}
#[test]
fn scene_export_text_and_rect() {
    let (mut b, id) = fixture();
    let source = b.p.add_item("Graphic", Label::Rose, ItemKind::Graphic { width: 1920, height: 1080, rate: FrameRate::FPS_24 }, None);
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.item = source;
    let mut text = fx(graphic::TEXT_LAYER);
    text.params.insert("text".into(), Param::new(ParamValue::Text("Title".into())));
    text.params.insert("box_width".into(), Param::new(ParamValue::Float(400.0)));
    text.params.insert("box_height".into(), Param::new(ParamValue::Float(200.0)));
    c.effects.push(text);
    c.effects.push(fx(graphic::SHAPE_LAYER));
    let (xml, _) = output(&b, id);
    assert_eq!(attr(&xml, "text", "text"), "Title");
    assert_eq!(attr(&xml, "shape", "shape"), "rect");
    assert_eq!(attr(&xml, "shape", "width"), "400");
    assert_eq!(attr(&xml, "shape", "height"), "200");
    assert_eq!(attr(&xml, "scene", "version"), "1.0");
}
#[test]
fn scene_export_nested_group() {
    let (mut b, inner) = fixture();
    let outer = b.reserve_sequence("Outer", SequenceSettings::default(), None);
    let mut track = b.track(TrackKind::Video, 0);
    track.items.push(b.clip(inner, TrackKind::Video, "Nest", t(5), t(2), t(1)));
    b.p.sequence_mut(outer).unwrap().video_tracks.push(track);
    let (xml, _) = output(&b, outer);
    // group child clock = start + (t - start - timeOffset) * timeScale.
    // At t=5 it must read child t=1, hence timeOffset=4 at speed 1.
    assert_eq!(attr(&xml, "group", "start"), "5");
    assert_eq!(attr(&xml, "group", "end"), "7");
    assert_eq!(attr(&xml, "group", "timeOffset"), "4");
    assert_eq!(attr(&xml, "group", "isolate"), "true");
    assert_eq!(attr(&xml, "layer", "start"), "0");
    assert_eq!(attr(&xml, "scene", "version"), "1.0");
}
#[test]
fn scene_export_rational_rate() {
    let (mut b, id) = fixture();
    b.p.sequence_mut(id).unwrap().settings.frame_rate = FrameRate::FPS_29_97;
    let (xml, _) = output(&b, id);
    assert_eq!(attr(&xml, "project", "fps"), "30000/1001");
}
#[test]
fn scene_export_empty_missing_and_hostile() {
    let (mut b, id) = fixture();
    b.p.sequence_mut(id).unwrap().video_tracks.clear();
    let (xml, _) = output(&b, id);
    // durationType is positive: an empty sequence gets one frame, 1/24 s.
    assert_eq!(values::seconds(&attr(&xml, "project", "duration")).unwrap(), FrameRate::FPS_24.tick_of(1));
    assert_eq!(export(&b.p, ItemId(u64::MAX), &ExportOptions::default(), &mut Report::default()), Err(Error::NoSequence(ItemId(u64::MAX))));
    for (start, duration, speed) in
        [(Tick(i64::MAX), t(1), 1.0), (t(0), Tick(i64::MAX), f64::MAX), (t(0), t(0), 1.0), (t(0), t(1), 0.0), (t(0), t(1), f64::NAN)]
    {
        let (mut b, id) = fixture();
        let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
        c.start = start;
        c.duration = duration;
        c.speed = speed;
        let (_, report) = output(&b, id);
        if duration == Tick::ZERO || !speed.is_finite() || start.0 == i64::MAX {
            assert!(report.has_warnings());
        }
    }
}
#[test]
fn scene_export_xml_escaping_paths() {
    let (mut b, id) = fixture();
    let seq = b.p.sequence_mut(id).unwrap();
    let c = &mut seq.video_tracks[0].items[0];
    c.name = "&<>\"'\t\n\r\u{1}".into();
    let media = c.item;
    if let ItemKind::Media(m) = &mut b.p.item_mut(media).unwrap().kind {
        m.media = filmcraft_project::MediaRef::File { path: "/media/&<>\"'\t.mov".into() };
    }
    let mut report = Report::default();
    let xml = export(&b.p, id, &ExportOptions { relative_to: Some("/media".into()), ..Default::default() }, &mut report).unwrap();
    checked(&xml);
    assert_eq!(attr(&xml, "layer", "name"), "&<>\"'\t\n\r�");
    assert!(report.mentions("XML"));
    assert_eq!(attr(&xml, "video", "src"), "&<>\"'\t.mov");
}
#[test]
fn scene_export_dispatch_and_detection() {
    assert_eq!(crate::Format::from_extension("SCENE"), Some(crate::Format::Scene));
    assert_eq!(crate::detect(b"<!-- <xmeml> --><?p x?><scene version='1.0'><project/></scene>", Some("xml")), Some(crate::Format::Scene));
    assert_ne!(crate::detect(b"<other><scene version='1.0'/></other>", None), Some(crate::Format::Scene));
    assert_ne!(crate::detect(b"<scene/>", None), Some(crate::Format::Scene));
    let (b, id) = fixture();
    let (bytes, _) = crate::export(&b.p, id, crate::Format::Scene, &ExportOptions::default()).unwrap();
    checked(std::str::from_utf8(&bytes).unwrap());
}
proptest! {
    #[test]
    fn scene_export_seconds_on_common_grids(sample in -345_599_999i64..345_600_000, grid in 0usize..4) {
        // Snap a time strictly inside ±2h to each exact integer frame/sample grid.
        let step = [TICKS_PER_SECOND/24, TICKS_PER_SECOND/25, TICKS_PER_SECOND*1001/30000, TICKS_PER_SECOND/48000][grid];
        let tick = Tick((sample * (TICKS_PER_SECOND/48000) / step) * step);
        let s = seconds(tick).unwrap();
        prop_assert_eq!(s.error, 0);
        prop_assert_eq!(values::seconds(&s.text).unwrap(), tick);
    }
}

#[test]
fn scene_export_audio_gain_mute_and_trim() {
    let (mut b, id) = fixture();
    let source = b.p.sequence(id).unwrap().video_tracks[0].items[0].item;
    let mut tr = b.track(TrackKind::Audio, 0);
    let mut c = b.clip(source, TrackKind::Audio, "Audio", t(2), t(3), t(5));
    c.speed = 2.0;
    c.gain_db = 6.0;
    c.effect_mut("volume").unwrap().params.get_mut("level").unwrap().value = ParamValue::Float(-12.0);
    tr.volume_db = 3.0;
    tr.pan = 25.0;
    tr.muted = true;
    tr.items.push(c);
    b.p.sequence_mut(id).unwrap().audio_tracks.push(tr);
    let (xml, _) = output(&b, id);
    // -12 + 6 + 3 = -3dB. audioTrack has no end; source [5,11) at speed 2 lasts 3s.
    assert_eq!(attr(&xml, "audioTrack", "start"), "2");
    assert_eq!(attr(&xml, "audioTrack", "clipOut"), "11");
    assert_eq!(attr(&xml, "audioTrack", "gain"), "-3");
    assert_eq!(attr(&xml, "audioTrack", "pan"), "0.25");
    assert_eq!(attr(&xml, "audioTrack", "mute"), "true");
    assert_eq!(attr(&xml, "layer", "mute"), "true"); // Video-track clips do not play a second copy of linked audio.
}
#[test]
fn scene_export_reports_unsupported_and_cycles() {
    let (mut b, id) = fixture();
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    let mut e = fx("opacity");
    e.effect = "unknown".into();
    c.effects.push(e.clone());
    c.effects.push(e);
    let (_, report) = output(&b, id);
    assert!(report.entries.iter().any(|e| e.message.contains("effect") && e.count == 2));
    b.p.sequence_mut(id).unwrap().video_tracks[0].items[0].item = id;
    let (_, report) = output(&b, id);
    assert!(report.mentions("cycle"));
}

#[test]
fn scene_export_nested_audio_clock() {
    let (mut b, inner) = fixture();
    let source = b.p.sequence(inner).unwrap().video_tracks[0].items[0].item;
    let mut audio = b.track(TrackKind::Audio, 0);
    audio.items.push(b.clip(source, TrackKind::Audio, "Inner audio", t(0), t(4), t(10)));
    b.p.sequence_mut(inner).unwrap().audio_tracks.push(audio);
    let outer = b.reserve_sequence("Outer", SequenceSettings::default(), None);
    let mut track = b.track(TrackKind::Audio, 0);
    let mut nest = b.clip(inner, TrackKind::Audio, "Nested audio", t(5), t(1), t(1));
    nest.speed = 2.0;
    track.items.push(nest);
    b.p.sequence_mut(outer).unwrap().audio_tracks.push(track);
    let (xml, report) = output(&b, outer);
    // The outer clip selects child [1,3), mapping it onto root [5,6).
    // Child source [10,14) is trimmed to [11,13), played at twice speed.
    for (key, value) in [("start", "5"), ("clipIn", "11"), ("clipOut", "13"), ("speed", "2")] {
        assert_eq!(attr(&xml, "audioTrack", key), value);
    }
    assert!(!report.mentions("nested audio clip dropped"));
}
#[test]
fn scene_export_shape_anchor_and_animation() {
    let (mut b, id) = fixture();
    let source = b.p.add_item("Graphic", Label::Rose, ItemKind::Graphic { width: 1920, height: 1080, rate: FrameRate::FPS_24 }, None);
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.item = source;
    let mut shape = fx(graphic::SHAPE_LAYER);
    shape.params.get_mut("anchor").unwrap().value = ParamValue::Vec2(Vec2::new(10.0, 20.0));
    shape.params.get_mut("opacity").unwrap().keyframes = vec![Keyframe::new(t(0), ParamValue::Float(25.0)), Keyframe::new(t(2), ParamValue::Float(75.0))];
    c.effects.push(shape);
    let (xml, _) = output(&b, id);
    // FilmCraft rect is [-200,200]x[-100,100], scene rect is [0,400]x[0,200].
    assert_eq!(attr(&xml, "shape", "anchorX"), "210");
    assert_eq!(attr(&xml, "shape", "anchorY"), "120");
    let doc = read::read(&xml, &Limits::default()).unwrap();
    let shape = doc.root().descendants().find(|n| n.has_tag_name("shape")).unwrap();
    let anim = shape.children().find(|n| n.has_tag_name("animate")).expect("shape opacity animation");
    assert_eq!(anim.attribute("timeBase"), Some("local"));
    assert_eq!(anim.children().find(|n| n.has_tag_name("key")).unwrap().attribute("value"), Some("0.25"));
}
#[test]
fn scene_export_hold_does_not_disappear() {
    let (mut b, id) = fixture();
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.frame_hold = Some(t(7));
    let (xml, _) = output(&b, id);
    // A hold has no source span requirement; clipOut still uses one timeline frame of source,
    // avoiding an unrepresentable 7s+1tick at very large hold times.
    assert_eq!(attr(&xml, "layer", "speed"), "0");
    assert_eq!(attr(&xml, "layer", "clipIn"), "7");
}
#[test]
fn scene_export_animated_limits_are_reported() {
    let (mut b, id) = fixture();
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap().keyframes = vec![Keyframe::new(t(0), ParamValue::Float(200.0))];
    let (xml, report) = output(&b, id);
    assert_eq!(attr(&xml, "key", "value"), "1");
    assert!(report.mentions("animation value"));
}
#[test]
fn scene_export_unrepresentable_tick_is_explicit() {
    // Above 2^53, double products cannot yield odd integer ticks at all.
    let tick = Tick((1_i64 << 53) + 1);
    let secs = seconds(tick).unwrap();
    assert_ne!(secs.error, 0);
    assert_eq!(values::seconds(&secs.text).unwrap().0 - tick.0, secs.error);
    let (b, id) = fixture();
    output(&b, id);
}
proptest! {
    #[test]
    fn scene_export_full_duration_nearest_or_exact(frame in 0i64..48_000_000_000, grid in 0usize..4) {
        let step = [TICKS_PER_SECOND/24, TICKS_PER_SECOND/25, TICKS_PER_SECOND*1001/30000, TICKS_PER_SECOND/48000][grid];
        let tick = Tick((frame % (MAX_TIME / step)) * step);
        let end = Tick(tick.0 + step);
        let (mut b, id) = fixture();
        let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
        c.start = tick;
        c.duration = Tick(step);
        let (xml, report) = output(&b, id);
        prop_assert!(xml.contains("<layer "));
        let start_back = values::seconds(&attr(&xml, "layer", "start")).unwrap();
        let end_back = values::seconds(&attr(&xml, "layer", "end")).unwrap();
        prop_assert!((start_back.0 - tick.0).abs() <= 64);
        prop_assert!((end_back.0 - end.0).abs() <= 64);
        // One 48kHz sample is 5,292,000 ticks, far larger than 2 * 64 ticks.
        prop_assert!(step >= 5_292_000);
        prop_assert!(end_back > start_back);
        let inexact = start_back != tick || end_back != end || seconds(Tick(step)).unwrap().error != 0;
        prop_assert_eq!(report.mentions("beyond exact f64 tick precision"), inexact);
    }
}

#[test]
fn scene_export_large_frame_precision_regression() {
    // 32768 + 1/24 s has adjacent doubles whose Tick results differ by ±1.
    // Tie goes downward: -1 tick. One second later has the same error.
    // Written inexact attributes: layer start, layer end, project duration = 3.
    let tick = Tick(t(32768).0 + TICKS_PER_SECOND / 24);
    let secs = seconds(tick).unwrap();
    assert_eq!(secs.error, -1);
    let (mut b, id) = fixture();
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.start = tick;
    c.duration = t(1);
    let (xml, report) = output(&b, id);
    assert!(xml.contains("<layer "));
    assert!((values::seconds(&attr(&xml, "layer", "start")).unwrap().0 - tick.0).abs() <= 1);
    assert!(report.mentions("3 time(s) beyond exact f64 tick precision written to the nearest representable value (max error 1 ticks)"), "{report}");
    assert_eq!(report.to_string().matches("beyond exact f64 tick precision").count(), 1);
}

#[test]
fn scene_export_collapsed_interval_is_reported() {
    // Both 2^53 and 2^53+1 round to 2^53 ticks under the lower tie policy.
    let (mut b, id) = fixture();
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.start = Tick(1_i64 << 53);
    c.duration = Tick(1);
    let (xml, report) = output(&b, id);
    assert!(!xml.contains("<layer "));
    assert!(report.mentions("collapsed"), "{report}");
}

#[test]
fn scene_export_seconds_range_and_nearest() {
    for tick in [Tick(-Tick::MAX.0), Tick::MAX, Tick(-((1_i64 << 53) + 1)), Tick(MAX_TIME), Tick(MAX_TIME - 1)] {
        let secs = seconds(tick).unwrap();
        let back = values::seconds(&secs.text).unwrap();
        assert_eq!(back.0 - tick.0, secs.error);
        let s = tick.0 as f64 / TICKS_PER_SECOND as f64;
        for candidate in [s.next_down(), s, s.next_up()] {
            if let Some(other) = values::seconds_to_tick(candidate) {
                assert!((secs.error.unsigned_abs(), back) <= ((other.0 - tick.0).unsigned_abs(), other));
            }
        }
    }
    for tick in [Tick(i64::MIN), Tick(i64::MAX), Tick(Tick::MAX.0 + 1), Tick(Tick::MIN.0 - 1)] {
        assert!(seconds(tick).is_err());
    }
    let (b, id) = fixture();
    output(&b, id);
}

#[test]
fn scene_export_path_shape() {
    let (mut b, id) = fixture();
    let source = b.p.add_item("Graphic", Label::Rose, ItemKind::Graphic { width: 1920, height: 1080, rate: FrameRate::FPS_24 }, None);
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.item = source;
    let mut path = fx(graphic::SHAPE_LAYER);
    path.params.get_mut("shape").unwrap().value = ParamValue::Choice(3);
    path.params.get_mut("points").unwrap().value = ParamValue::Curve(vec![[0.0, 0.0], [20.0, 0.0], [0.0, 10.0]]);
    c.effects.push(path);
    let (xml, _) = output(&b, id);
    // FilmCraft path points already use layer-local coordinates: no rectangle-centre offset.
    assert_eq!(attr(&xml, "shape", "shape"), "path");
    assert_eq!(attr(&xml, "shape", "path"), "M0,0 L20,0 L0,10 Z");
    assert_eq!(attr(&xml, "shape", "anchorX"), "0");
}
#[test]
fn scene_export_animated_audio_can_rise_from_silence() {
    let (mut b, id) = fixture();
    let source = b.p.sequence(id).unwrap().video_tracks[0].items[0].item;
    let mut track = b.track(TrackKind::Audio, 0);
    let mut c = b.clip(source, TrackKind::Audio, "Fade in", t(0), t(4), t(0));
    let param = c.effect_mut("volume").unwrap().params.get_mut("level").unwrap();
    param.value = ParamValue::Float(-200.0);
    param.keyframes = vec![Keyframe::new(t(0), ParamValue::Float(-200.0)), Keyframe::new(t(2), ParamValue::Float(0.0))];
    track.items.push(c);
    b.p.sequence_mut(id).unwrap().audio_tracks.push(track);
    let (xml, _) = output(&b, id);
    assert_eq!(attr(&xml, "audioTrack", "mute"), "false");
    assert_eq!(attr(&xml, "key", "value"), "-120");
}
#[test]
fn scene_export_large_hold_uses_no_artificial_tick_span() {
    let (mut b, id) = fixture();
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.frame_hold = Some(t(40000));
    let (xml, _) = output(&b, id);
    assert_eq!(attr(&xml, "layer", "clipIn"), "40000");
    assert_eq!(attr(&xml, "layer", "speed"), "0");
}

#[test]
fn scene_export_detection_utf8_at_sniff_boundary() {
    let (b, id) = fixture();
    let (xml, _) = output(&b, id);
    // Only the root start tag is needed. A later UTF-8 character cut by the 4KiB
    // sniff window must not hide an otherwise recognizable root.
    let start = xml.find("<scene").unwrap();
    let end = xml[start..].find('>').unwrap() + start + 1;
    let mut head = xml[..end].to_owned();
    head.push_str(&" ".repeat(4095 - head.len()));
    head.push('é');
    head.push_str("</scene>");
    assert_eq!(crate::detect(head.as_bytes(), None), Some(crate::Format::Scene));
}

fn imprecise_time() -> Tick {
    Tick(t(32768).0 + TICKS_PER_SECOND / 24)
}

fn near_time(xml: &str, tag: &str, attribute: &str, target: Tick) {
    assert!((values::seconds(&attr(xml, tag, attribute)).unwrap().0 - target.0).abs() <= 1);
}

#[test]
fn scene_export_inexact_asset_duration() {
    let (mut b, id) = fixture();
    let source = b.p.sequence(id).unwrap().video_tracks[0].items[0].item;
    b.p.item_mut(source).unwrap().as_media_mut().unwrap().info.duration = imprecise_time();
    let (xml, report) = output(&b, id);
    near_time(&xml, "video", "duration", imprecise_time());
    assert!(xml.contains("<layer "));
    assert!(report.mentions("1 time(s) beyond exact f64 tick precision"));
}

#[test]
fn scene_export_inexact_source_and_animation() {
    let (mut b, id) = fixture();
    let c = &mut b.p.sequence_mut(id).unwrap().video_tracks[0].items[0];
    c.source_in = imprecise_time();
    c.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap().keyframes = vec![Keyframe::new(imprecise_time(), ParamValue::Float(50.0))];
    let (xml, report) = output(&b, id);
    // Source in/out and one key are inexact; all timeline edges are integers.
    near_time(&xml, "layer", "clipIn", imprecise_time());
    near_time(&xml, "layer", "clipOut", imprecise_time() + t(4));
    near_time(&xml, "key", "time", imprecise_time());
    assert!(report.mentions("3 time(s) beyond exact f64 tick precision"));
}

#[test]
fn scene_export_inexact_nested_clock() {
    let (mut b, inner) = fixture();
    let outer = b.reserve_sequence("Outer", SequenceSettings::default(), None);
    let mut track = b.track(TrackKind::Video, 0);
    track.items.push(b.clip(inner, TrackKind::Video, "Nest", imprecise_time(), t(2), t(0)));
    b.p.sequence_mut(outer).unwrap().video_tracks.push(track);
    let (xml, report) = output(&b, outer);
    near_time(&xml, "group", "timeOffset", imprecise_time());
    assert!(xml.contains("<layer "));
    // group start/end/offset and project duration; child clocks are exact.
    assert!(report.mentions("4 time(s) beyond exact f64 tick precision"));
}

#[test]
fn scene_export_inexact_transition() {
    let (mut b, id) = fixture();
    let track = &mut b.p.sequence_mut(id).unwrap().video_tracks[0];
    track.items[0].duration = t(40000);
    track.transitions.push(Transition {
        id: TransitionId(51),
        effect: fx("cross_dissolve"),
        start: t(40000) - imprecise_time(),
        duration: imprecise_time(),
        from: Some(track.items[0].id),
        to: None,
        align: TransitionAlign::EndAtCut,
        reverse: false,
    });
    let (xml, report) = output(&b, id);
    near_time(&xml, "transition", "duration", imprecise_time());
    assert!(report.mentions("1 time(s) beyond exact f64 tick precision"));
}

#[test]
fn scene_export_inexact_audio_and_metadata() {
    let (mut b, id) = fixture();
    let source = b.p.sequence(id).unwrap().video_tracks[0].items[0].item;
    let mut audio = b.track(TrackKind::Audio, 0);
    audio.items.push(b.clip(source, TrackKind::Audio, "Audio", imprecise_time(), t(1), imprecise_time()));
    let seq = b.p.sequence_mut(id).unwrap();
    seq.audio_tracks.push(audio);
    seq.markers.push(Marker {
        id: MarkerId(70),
        start: imprecise_time(),
        duration: imprecise_time(),
        name: "Mark".into(),
        comment: String::new(),
        kind: MarkerKind::Chapter,
        color: Label::Blue,
    });
    let mut captions = CaptionTrack::new(TrackId(80), "Captions".into(), Default::default());
    captions.language = "en".into();
    captions.captions.push(Caption {
        id: ClipId(81),
        start: imprecise_time(),
        duration: t(1),
        text: "Hello".into(),
        speaker: None,
        cue_id: None,
        settings: String::new(),
    });
    seq.caption_tracks.push(captions);
    let (xml, report) = output(&b, id);
    for (tag, name) in [("audioTrack", "start"), ("audioTrack", "clipIn"), ("marker", "time"), ("marker", "duration"), ("cue", "start")] {
        near_time(&xml, tag, name, imprecise_time());
    }
    // Audio 3, marker 2, cue 2, project duration 1 = 8 written inexact times.
    assert!(report.mentions("8 time(s) beyond exact f64 tick precision"), "{report}");
    assert_eq!(report.to_string().matches("beyond exact f64 tick precision").count(), 1);
}
