//! Bidirectional scene interchange, with hand-derived edit and source-clock expectations.
#![cfg(test)]

mod support;

use filmcraft_geom::Vec2;
use filmcraft_interchange::{ExportOptions, Format, Imported, Level, Report, scene};
use filmcraft_project::{
    Caption, CaptionTrack, ClipId, Interpolation, ItemId, ItemKind, Keyframe, Label, Marker, MarkerId, MarkerKind, ParamValue, Project, Sequence,
    SequenceSettings, TrackId, TrackItem, TrackKind, TransitionAlign, find_effect, graphic,
};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick, TimeRange};
use proptest::prelude::*;

fn sec(n: i64) -> Tick {
    Tick(n * TICKS_PER_SECOND)
}

fn project(rate: FrameRate, video: usize, audio: usize) -> (Project, ItemId, ItemId) {
    let mut p = Project::new("Round trip");
    let id = p.new_sequence("Sequence", SequenceSettings { frame_rate: rate, ..Default::default() }, video, audio, None);
    let source = support::media(&mut p, if video > 0 { "/media/video.mov" } else { "/media/audio.wav" }, video > 0, audio > 0, rate);
    p.item_mut(source).unwrap().as_media_mut().unwrap().info.duration = sec(14_400);
    (p, id, source)
}

fn place(p: &mut Project, seq: ItemId, source: ItemId, kind: TrackKind, track: usize, start: Tick, duration: Tick, input: Tick) -> ClipId {
    let rate = p.sequence(seq).unwrap().settings.frame_rate;
    let mut c = p.make_track_item(source, kind, start, TimeRange::new(input, duration), rate).unwrap();
    // make_track_item snaps durations for UI edits; audio fixtures deliberately use sample edges.
    c.duration = duration;
    let id = c.id;
    for e in &mut c.effects {
        filmcraft_project::resolve_auto_points(e, (1920, 1080), (1920, 1080));
    }
    p.sequence_mut(seq).unwrap().tracks_mut(kind)[track].items.push(c);
    id
}

fn fixture() -> (Project, ItemId, ClipId) {
    let (mut p, id, source) = project(FrameRate::FPS_24, 1, 0);
    let clip = place(&mut p, id, source, TrackKind::Video, 0, sec(0), sec(4), sec(0));
    (p, id, clip)
}

fn checked(xml: &str) {
    let d = scene::read::read(xml, &Default::default()).unwrap();
    let errors: Vec<_> = scene::validate::validate(&d.doc).into_iter().filter(|d| d.is_error()).collect();
    assert!(errors.is_empty(), "{errors:?}\n{xml}");
}

fn export_scene(p: &Project, id: ItemId) -> (String, Report) {
    let (bytes, report) = filmcraft_interchange::export(p, id, Format::Scene, &ExportOptions::default()).unwrap();
    let xml = String::from_utf8(bytes).unwrap();
    checked(&xml);
    (xml, report)
}

fn import_scene(xml: &str) -> (Imported, Report) {
    checked(xml);
    let (p, overlays, report) = scene::import(xml.as_bytes(), &Default::default()).unwrap();
    assert!(overlays.is_empty(), "unexpected overlays: {overlays:?}");
    (p, report)
}

fn roundtrip(p: &Project, id: ItemId) -> Imported {
    let (xml, _) = export_scene(p, id);
    import_scene(&xml).0
}

fn sequence(p: &Imported) -> &Sequence {
    assert_eq!(p.sequences.len(), 1);
    p.project.sequence(p.sequences[0]).unwrap()
}

fn content(s: &Sequence, kind: TrackKind) -> Vec<Vec<&TrackItem>> {
    s.tracks(kind).iter().map(|t| t.items.iter().filter(|c| c.enabled && c.name != "Scene Background").collect::<Vec<_>>()).filter(|c| !c.is_empty()).collect()
}

fn assert_clip(a: &TrackItem, b: &TrackItem) {
    assert_eq!((b.start, b.end(), b.duration), (a.start, a.end(), a.duration), "clip intervals");
    assert_eq!(b.source_in, a.source_in, "source in-point");
    assert_eq!(b.speed, a.speed, "speed");
    assert_eq!(b.reverse, a.reverse);
}

fn assert_edit(p: &Project, id: ItemId, q: &Imported) {
    let a = p.sequence(id).unwrap();
    let b = sequence(q);
    assert_eq!(b.settings.frame_rate, a.settings.frame_rate);
    assert_eq!((b.settings.width, b.settings.height), (a.settings.width, a.settings.height));
    assert_eq!(b.duration(), a.duration());
    // XSD projectType@background defaults to opaque black, materialized as a bottom track.
    assert_eq!(b.video_tracks.len(), a.video_tracks.len() + 1, "video tracks including scene background");
    assert_eq!(b.audio_tracks.len(), a.audio_tracks.len(), "audio tracks");
    for kind in [TrackKind::Video, TrackKind::Audio] {
        let (aa, bb) = (content(a, kind), content(b, kind));
        assert_eq!(bb.len(), aa.len(), "content track count");
        for (aa, bb) in aa.iter().zip(bb) {
            assert_eq!(bb.len(), aa.len(), "clip count");
            for (a, b) in aa.iter().zip(bb) {
                assert_clip(a, b);
            }
        }
    }
}

fn close(a: f64, b: f64) {
    assert!(a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-6, "{a} != {b}");
}

fn value(a: &ParamValue, b: &ParamValue) {
    match (a, b) {
        (ParamValue::Float(a), ParamValue::Float(b)) => close(*a, *b),
        (ParamValue::Vec2(a), ParamValue::Vec2(b)) => {
            close(a.x, b.x);
            close(a.y, b.y);
        }
        _ => assert_eq!(a, b),
    }
}

#[test]
fn filmcraft_one_clip() {
    let (p, id, _) = fixture();
    // [0,4), source [0,4), one content track plus the imported black background.
    assert_edit(&p, id, &roundtrip(&p, id));
}

#[test]
fn filmcraft_trimmed_speed_two() {
    let (mut p, id, clip) = fixture();
    let c = p.sequence_mut(id).unwrap().find_item_mut(clip).unwrap().1;
    // [2,5) at 2x consumes source [5,11).
    c.start = sec(2);
    c.duration = sec(3);
    c.source_in = sec(5);
    c.speed = 2.;
    assert_edit(&p, id, &roundtrip(&p, id));
}

#[test]
fn filmcraft_two_overlapping_tracks() {
    let (mut p, id, source) = project(FrameRate::FPS_24, 2, 0);
    // Lower [0,5), upper [2,6): paint order requires two content tracks.
    place(&mut p, id, source, TrackKind::Video, 0, sec(0), sec(5), sec(1));
    place(&mut p, id, source, TrackKind::Video, 1, sec(2), sec(4), sec(7));
    assert_edit(&p, id, &roundtrip(&p, id));
}

#[test]
fn filmcraft_motion_with_anchor() {
    let (mut p, id, clip) = fixture();
    let m = p.sequence_mut(id).unwrap().find_item_mut(clip).unwrap().1.effect_mut("motion").unwrap();
    // T(101.25,202.5) R(30) S(1.5,2) T(-13.25,-27.5), in natural source pixels.
    for (name, v) in [
        ("position", ParamValue::Vec2(Vec2::new(101.25, 202.5))),
        ("anchor", ParamValue::Vec2(Vec2::new(13.25, 27.5))),
        ("scale", ParamValue::Float(200.)),
        ("scale_width", ParamValue::Float(150.)),
        ("uniform_scale", ParamValue::Bool(false)),
        ("rotation", ParamValue::Float(30.)),
    ] {
        m.params.get_mut(name).unwrap().value = v;
    }
    let q = roundtrip(&p, id);
    assert_edit(&p, id, &q);
    let a = p.sequence(id).unwrap().find_item(clip).unwrap().1.effect("motion").unwrap();
    let b = content(sequence(&q), TrackKind::Video)[0][0].effect("motion").unwrap();
    for name in ["position", "anchor", "scale", "scale_width", "rotation"] {
        value(&a.param(name).unwrap().value, &b.param(name).unwrap().value);
    }
}

fn opacity_case(trimmed: bool, compare_curve: bool) {
    let (mut p, id, clip) = fixture();
    let c = p.sequence_mut(id).unwrap().find_item_mut(clip).unwrap().1;
    c.start = sec(5);
    if trimmed {
        c.source_in = sec(2);
        c.speed = 2.;
    }
    let first = c.source_in;
    let last = first + sec(if trimmed { 8 } else { 4 });
    let param = c.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap();
    param.keyframes = vec![Keyframe::new(first, ParamValue::Float(20.)), Keyframe::new(last, ParamValue::Float(80.))];
    param.keyframes[0].interp = Interpolation::EaseOut;
    param.keyframes[1].interp = Interpolation::EaseIn;
    let q = roundtrip(&p, id);
    assert_edit(&p, id, &q);
    let a = p.sequence(id).unwrap().find_item(clip).unwrap().1;
    let b = content(sequence(&q), TrackKind::Video)[0][0];
    let ap = a.effect("opacity").unwrap().param("opacity").unwrap();
    let bp = b.effect("opacity").unwrap().param("opacity").unwrap();
    assert_eq!(bp.keyframes.len(), 2);
    for (a, b) in ap.keyframes.iter().zip(&bp.keyframes) {
        assert_eq!(b.time, a.time, "opacity key time");
        value(&a.value, &b.value);
    }
    // EaseOut + EaseIn is the cubic 3u²-2u³: at 1/4, 20 + 60*5/32 = 29.375.
    for (offset, expected) in [(0, 20.), (1, 29.375), (2, 50.), (3, 70.625), (4, 80.)] {
        let time = sec(5 + offset);
        close(ap.f64_at(a.effect_time_at(time)), expected);
        if compare_curve {
            close(bp.f64_at(b.effect_time_at(time)), expected);
        }
    }
}

#[test]
fn filmcraft_opacity_ease() {
    opacity_case(false, false);
}

#[test]
fn filmcraft_opacity_ease_curve() {
    opacity_case(false, true);
}

#[test]
fn filmcraft_trimmed_speed_opacity_ease() {
    opacity_case(true, true);
}

#[test]
fn filmcraft_cross_dissolve() {
    let (mut p, id, source) = project(FrameRate::FPS_24, 1, 0);
    let a = place(&mut p, id, source, TrackKind::Video, 0, sec(0), sec(4), sec(2));
    let b = place(&mut p, id, source, TrackKind::Video, 0, sec(4), sec(4), sec(2));
    // Centered two-second dissolve at cut 4 has half-open window [3,5).
    support::transition(&mut p, id, TrackKind::Video, 0, "cross_dissolve", Some(a), Some(b), 72, 48, TransitionAlign::CenterAtCut);
    let q = roundtrip(&p, id);
    assert_edit(&p, id, &q);
    let tracks: Vec<_> = sequence(&q).video_tracks.iter().filter(|t| !t.transitions.is_empty()).collect();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].transitions.len(), 1);
    let tr = &tracks[0].transitions[0];
    assert_eq!((tr.start, tr.duration, tr.align), (sec(3), sec(2), TransitionAlign::CenterAtCut));
    assert_eq!(tr.effect.effect, "cross_dissolve");
    assert_eq!(tr.from, Some(tracks[0].items[0].id));
    assert_eq!(tr.to, Some(tracks[0].items[1].id));
}

#[test]
fn filmcraft_markers() {
    let (mut p, id, _) = fixture();
    for (i, start, duration, kind, color) in [(0, 1, 0, MarkerKind::Comment, Label::Green), (1, 2, 1, MarkerKind::Chapter, Label::Blue)] {
        p.sequence_mut(id).unwrap().markers.push(Marker {
            id: MarkerId(100 + i),
            start: sec(start),
            duration: sec(duration),
            name: format!("M & <{i}>"),
            comment: String::new(),
            kind,
            color,
        });
    }
    let q = roundtrip(&p, id);
    assert_edit(&p, id, &q);
    let a = &p.sequence(id).unwrap().markers;
    let b = &sequence(&q).markers;
    assert_eq!(b.len(), a.len());
    for (a, b) in a.iter().zip(b) {
        // Scene stores RGB, not FilmCraft's label enum; Green and Forest share marker RGB.
        assert_eq!((b.start, b.duration, &b.name, b.kind, b.color.marker_rgb()), (a.start, a.duration, &a.name, a.kind, a.color.marker_rgb()));
    }
}

#[test]
fn filmcraft_captions() {
    let (mut p, id, _) = fixture();
    let mut track = CaptionTrack::new(TrackId(100), "Dialogue".into(), Default::default());
    track.language = "pt-BR".into();
    for (i, text) in [(0, "Olá & <hello>"), (1, "Second line\nNext line")] {
        track.captions.push(Caption {
            id: ClipId(101 + i),
            start: sec(i as i64 + 1),
            duration: sec(1),
            text: text.into(),
            speaker: Some("Ada".into()),
            cue_id: None,
            settings: String::new(),
        });
    }
    p.sequence_mut(id).unwrap().caption_tracks.push(track);
    let q = roundtrip(&p, id);
    assert_edit(&p, id, &q);
    assert_eq!(sequence(&q).caption_tracks.len(), 1);
    let a = &p.sequence(id).unwrap().caption_tracks[0];
    let b = &sequence(&q).caption_tracks[0];
    assert_eq!((&b.name, &b.language), (&a.name, &a.language));
    assert_eq!(b.captions.len(), a.captions.len());
    for (a, b) in a.captions.iter().zip(&b.captions) {
        assert_eq!((b.start, b.duration, &b.text, &b.speaker), (a.start, a.duration, &a.text, &a.speaker));
    }
}

#[test]
fn filmcraft_text_and_rect() {
    let (mut p, id, clip) = fixture();
    let source = p.add_item("Graphic", Label::Rose, ItemKind::Graphic { width: 1920, height: 1080, rate: FrameRate::FPS_24 }, None);
    let c = p.sequence_mut(id).unwrap().find_item_mut(clip).unwrap().1;
    c.item = source;
    for (kind, name) in [(graphic::TEXT_LAYER, "Title"), (graphic::SHAPE_LAYER, "Rectangle")] {
        let mut e = find_effect(kind).unwrap().instance();
        e.params.get_mut("name").unwrap().value = ParamValue::Text(name.into());
        e.params.get_mut("position").unwrap().value = ParamValue::Vec2(Vec2::new(300., 200.));
        e.params.get_mut("anchor").unwrap().value = ParamValue::Vec2(Vec2::new(10., 20.));
        if kind == graphic::TEXT_LAYER {
            e.params.get_mut("text").unwrap().value = ParamValue::Text("Title & subtitle".into());
            e.params.get_mut("box_width").unwrap().value = ParamValue::Float(400.);
            e.params.get_mut("box_height").unwrap().value = ParamValue::Float(200.);
        }
        c.effects.push(e);
    }
    let q = roundtrip(&p, id);
    assert_eq!(sequence(&q).video_tracks.len(), 2);
    let carrier = content(sequence(&q), TrackKind::Video)[0][0];
    assert_clip(p.sequence(id).unwrap().find_item(clip).unwrap().1, carrier);
    let inner = q.project.sequence(carrier.item).unwrap();
    assert_eq!(content(inner, TrackKind::Video).len(), 2);
    let layers: Vec<_> = inner.video_tracks.iter().flat_map(|t| &t.items).filter(|c| c.enabled).collect();
    assert_eq!(layers.len(), 2);
    for (a, b) in p.sequence(id).unwrap().find_item(clip).unwrap().1.graphic_layers().zip(layers) {
        assert_eq!((b.start, b.duration), (sec(0), sec(4)));
        let b = b.graphic_layers().next().unwrap();
        assert_eq!(b.effect, a.effect);
        let names: &[&str] = if a.effect == graphic::TEXT_LAYER {
            &["name", "text", "font", "size", "align", "box_width", "box_height", "position", "anchor", "opacity", "fill_color"]
        } else {
            &["name", "shape", "size", "position", "anchor", "opacity", "fill_color"]
        };
        for name in names {
            value(&a.param(name).unwrap().value, &b.param(name).unwrap().value);
        }
    }
}

#[test]
fn filmcraft_nested_sequence() {
    let (mut p, inner, _) = fixture();
    let outer = p.new_sequence("Outer", p.sequence(inner).unwrap().settings.clone(), 1, 0, None);
    // Untrimmed nest at 5 preserves child [0,4) and maps it to root [5,9).
    place(&mut p, outer, inner, TrackKind::Video, 0, sec(5), sec(4), sec(0));
    let q = roundtrip(&p, outer);
    assert_edit(&p, outer, &q);
    let carrier = content(sequence(&q), TrackKind::Video)[0][0];
    let nested = q.project.sequence(carrier.item).unwrap();
    assert_eq!(content(nested, TrackKind::Video).len(), 1);
    assert_clip(&p.sequence(inner).unwrap().video_tracks[0].items[0], content(nested, TrackKind::Video)[0][0]);
}

#[test]
fn filmcraft_2997_exact_ticks() {
    let rate = FrameRate::FPS_29_97;
    let (mut p, id, source) = project(rate, 1, 0);
    // Frame 1001 to 1124, source frame 17: each frame is 8,475,667,200 ticks.
    place(&mut p, id, source, TrackKind::Video, 0, Tick(8_475_667_200 * 1001), Tick(8_475_667_200 * 123), Tick(8_475_667_200 * 17));
    assert_edit(&p, id, &roundtrip(&p, id));
}

fn document(nodes: &str) -> String {
    format!(
        r##"<scene version="1.7"><project width="640" height="480" fps="25" duration="12" background="#00000000"/><assets><video id="v" src="v.mp4" width="640" height="480" fps="25" duration="30" hasAudio="false"/><image id="i" src="i.png" width="100" height="80"/><text id="txt" text="Hello" font="Inter" size="32" width="200" height="80"/></assets><composition>{nodes}</composition></scene>"##
    )
}

#[derive(Debug)]
struct Layer {
    kind: String,
    asset: String,
    start: Tick,
    end: Tick,
    span_start: Tick,
    span_end: Tick,
    segments: Vec<(Tick, Tick, Tick, Tick)>,
}

fn resolved_layers(xml: &str) -> Vec<Layer> {
    use scene::resolve::{NodeClass, SourceMap};
    let doc = scene::read::read(xml, &Default::default()).unwrap();
    let r = scene::resolve::resolve(&doc, &Default::default()).unwrap();
    let tick = |s| scene::values::seconds_to_tick(s).unwrap();
    let mut layers = Vec::new();
    for n in &r.nodes {
        let kind = match &n.class {
            NodeClass::Media(k) => format!("{k:?}"),
            NodeClass::Text => "text".into(),
            NodeClass::Shape(k) => k.clone(),
            _ => continue,
        };
        let asset = n
            .attributes
            .get("asset")
            .and_then(|id| doc.doc.descendants().find(|a| a.attribute("id") == Some(id)))
            .and_then(|a| a.attribute("src"))
            .unwrap_or("")
            .to_owned();
        let mut segments = Vec::new();
        if let Some(source) = &n.source {
            let SourceMap::Segments(ss) = source else { panic!("fixture must have an affine source map") };
            for s in ss {
                let start = s.comp_start.max(n.span.start);
                let end = s.comp_end.min(n.span.end);
                if end > start {
                    segments.push((
                        tick(start),
                        tick(end),
                        tick(s.src_start + (start - s.comp_start) * s.rate),
                        tick(s.src_start + (end - s.comp_start) * s.rate),
                    ));
                }
            }
        }
        layers.push(Layer {
            kind,
            asset,
            start: tick(n.interval.start),
            end: tick(n.interval.end),
            span_start: tick(n.span.start),
            span_end: tick(n.span.end),
            segments,
        });
    }
    layers.sort_by_key(|l| (l.start, l.end, l.kind.clone(), l.asset.clone()));
    layers
}

fn within_tick(a: Tick, b: Tick) {
    assert!((i128::from(a.0) - i128::from(b.0)).abs() <= 1, "{a:?} != {b:?}");
}

fn scene_case(nodes: &str, expected: &[(i64, i64, i64, i64)]) {
    let xml = document(nodes);
    let (p, report) = import_scene(&xml);
    // These source-bound observations lose no content; every other warning fails the fixture.
    assert!(
        report
            .entries
            .iter()
            .all(|e| e.level != Level::Warning || e.message.starts_with("MEDIA_DURATION_INFERRED:") || e.message.starts_with("SOURCE_END_EXTENDED:")),
        "fixture must map fully: {report}"
    );
    let a = resolved_layers(&xml);
    assert_eq!(a.len(), expected.len());
    for (a, &(start, end, source_start, source_end)) in a.iter().zip(expected) {
        assert_eq!((a.start, a.end), (sec(start), sec(end)), "hand-derived interval");
        if let Some(s) = a.segments.first() {
            within_tick(s.2, sec(source_start));
            within_tick(a.segments.last().unwrap().3, sec(source_end));
        }
    }
    let (out, _) = export_scene(&p.project, p.sequences[0]);
    checked(&out);
    let b = resolved_layers(&out);
    assert_eq!(b.len(), a.len(), "resolved layer count: {out}");
    for (a, b) in a.iter().zip(b) {
        assert_eq!((&b.kind, &b.asset), (&a.kind, &a.asset));
        within_tick(a.start, b.start);
        within_tick(a.end, b.end);
        within_tick(a.span_start, b.span_start);
        within_tick(a.span_end, b.span_end);
        assert_eq!(b.segments.len(), a.segments.len());
        for (a, b) in a.segments.iter().zip(b.segments) {
            for (a, b) in [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)] {
                within_tick(a, b);
            }
        }
    }
}

#[test]
fn scene_video_trim_speed() {
    // [1,4) consumes six source seconds starting at 2: [2,8).
    scene_case(r#"<layer id="a" asset="v" start="1" end="4" clipIn="2" clipOut="8" speed="2"/>"#, &[(1, 4, 2, 8)]);
}

#[test]
fn scene_overlapping_layers() {
    // Two independent layer clocks; the image's explicit source out defines its otherwise unknown span.
    scene_case(r#"<layer id="a" asset="v" end="5"/><layer id="b" asset="i" start="2" end="6" clipOut="4"/>"#, &[(0, 5, 0, 5), (2, 6, 0, 4)]);
}

#[test]
fn scene_anchor_and_opacity() {
    scene_case(
        r#"<layer id="a" asset="v" start="2" end="6" x="100" y="200" anchorX="10" anchorY="20"><animate property="opacity" timeBase="local"><key time="0" value="0"/><key time="4" value="1"/></animate></layer>"#,
        &[(2, 6, 0, 4)],
    );
}

#[test]
fn scene_text_and_rect() {
    // Shapes and text have no external media source; their common visible interval is [0,2).
    scene_case(r#"<layer id="t" asset="txt" end="2"/><shape id="r" shape="rect" width="100" height="80" end="2"/>"#, &[(0, 2, 0, 2), (0, 2, 0, 2)]);
}

#[test]
fn scene_nested_clock() {
    // group clock comp = 1 + local/2; child [2,6) becomes [2,4), source [1,5).
    scene_case(r#"<group id="g" start="2" end="6" timeScale="2"><layer id="a" asset="v" start="2" end="6" clipIn="1" clipOut="5"/></group>"#, &[(2, 4, 1, 5)]);
}

#[test]
fn scene_sequence_autoplacement() {
    // First [0,2); timeGap 1 places the next two-second source at [3,5).
    scene_case(
        r#"<sequence id="s" timeGap="1"><layer id="a" asset="v" end="2" clipOut="2"/><layer id="b" asset="v" end="2" clipIn="4" clipOut="6"/></sequence>"#,
        &[(0, 2, 0, 2), (3, 5, 4, 6)],
    );
}

#[test]
fn scene_dissolve_source_maps() {
    // Nominal [0,4), [4,8), centered window [3,5); resolver extends media handles.
    scene_case(
        r#"<layer id="a" asset="v" end="4" clipIn="2" clipOut="6"/><layer id="b" asset="v" start="4" end="8" clipIn="2" clipOut="6"/><transition from="a" to="b" type="crossfade" duration="2" curve="linear"/>"#,
        &[(0, 4, 2, 7), (4, 8, 1, 6)],
    );
}

fn random_roundtrip(grid: usize, entries: &[(u32, u32, u32)]) {
    let rate = [FrameRate::FPS_24, FrameRate::FPS_25, FrameRate::FPS_29_97, FrameRate::FPS_25][grid];
    let audio = grid == 3;
    let step = [TICKS_PER_SECOND / 24, TICKS_PER_SECOND / 25, TICKS_PER_SECOND * 1001 / 30000, TICKS_PER_SECOND / 48000][grid];
    let kind = if audio { TrackKind::Audio } else { TrackKind::Video };
    let (mut p, id, source) = project(rate, usize::from(!audio), usize::from(audio));
    // Partition two hours into one slot per clip: arbitrary gaps and trims, never overlapping.
    let slot = sec(7200).0 / step / entries.len() as i64;
    for (i, &(gap, len, input)) in entries.iter().enumerate() {
        let gap = i64::from(gap) % (slot - 1);
        let duration = 1 + i64::from(len) % (slot - gap);
        place(&mut p, id, source, kind, 0, Tick((i as i64 * slot + gap) * step), Tick(duration * step), Tick((i64::from(input) % slot) * step));
    }
    let q = roundtrip(&p, id);
    let a: Vec<_> = p.sequence(id).unwrap().tracks(kind).iter().flat_map(|t| &t.items).map(|c| (c.start, c.end(), c.duration)).collect();
    let mut b: Vec<_> = sequence(&q)
        .tracks(kind)
        .iter()
        .flat_map(|t| &t.items)
        .filter(|c| c.enabled && c.name != "Scene Background")
        .map(|c| (c.start, c.end(), c.duration))
        .collect();
    b.sort();
    assert_eq!(b, a, "grid {grid}, entries {entries:?}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, failure_persistence: None, ..ProptestConfig::default() })]
    #[test]
    fn random_24fps(entries in prop::collection::vec((any::<u32>(), any::<u32>(), any::<u32>()), 1..9)) { random_roundtrip(0, &entries); }
    #[test]
    fn random_25fps(entries in prop::collection::vec((any::<u32>(), any::<u32>(), any::<u32>()), 1..9)) { random_roundtrip(1, &entries); }
    #[test]
    fn random_2997fps(entries in prop::collection::vec((any::<u32>(), any::<u32>(), any::<u32>()), 1..9)) { random_roundtrip(2, &entries); }
    #[test]
    fn random_48khz(entries in prop::collection::vec((any::<u32>(), any::<u32>(), any::<u32>()), 1..9)) { random_roundtrip(3, &entries); }
}

#[test]
fn filmcraft_all_eases_at_frames_forward_and_reverse() {
    for reverse in [false, true] {
        for interpolation in Interpolation::ALL {
            let (mut p, id, clip) = fixture();
            let c = p.sequence_mut(id).unwrap().find_item_mut(clip).unwrap().1;
            c.source_in = sec(4);
            c.speed = 2.;
            c.reverse = reverse;
            let param = c.effect_mut("opacity").unwrap().params.get_mut("opacity").unwrap();
            // Four seconds of video consume [4,12). Off-interval endpoints also exercise retention.
            param.keyframes = vec![
                Keyframe::new(sec(2), ParamValue::Float(10.)),
                Keyframe::new(sec(8), ParamValue::Float(90.)),
                Keyframe::new(sec(14), ParamValue::Float(30.)),
            ];
            for key in &mut param.keyframes {
                key.interp = interpolation;
                key.in_influence = 0.4;
                key.out_influence = 0.2;
            }
            let q = roundtrip(&p, id);
            let a = p.sequence(id).unwrap().find_item(clip).unwrap().1;
            let b = content(sequence(&q), TrackKind::Video)[0][0];
            for frame in 0..96 {
                let time = a.start + Tick(TICKS_PER_SECOND * frame / 24);
                let av = a.effect("opacity").unwrap().param("opacity").unwrap().f64_at(a.effect_time_at(time));
                let bv = b.effect("opacity").unwrap().param("opacity").unwrap().f64_at(b.effect_time_at(time));
                assert!((av - bv).abs() < 1e-6, "{interpolation:?} reverse={reverse} frame={frame}: {av} != {bv}");
            }
        }
    }
}
