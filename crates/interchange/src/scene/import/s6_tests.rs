use super::*;
use filmcraft_geom::Vec2;
use filmcraft_project::{ParamValue, graphic};
use filmcraft_time::TICKS_PER_SECOND;
use proptest::prelude::*;
fn sec(n: i64) -> Tick {
    Tick(n * TICKS_PER_SECOND)
}
fn scene(nodes: &str, extra: &str) -> String {
    let (before, after) = extra.find("<audioMix").or_else(|| extra.find("<captions")).map_or((extra, ""), |i| extra.split_at(i));
    format!(
        r##"<scene version="1.7"><project width="640" height="480" fps="25" duration="12" background="#00000000"/><assets><image id="i" src="i.png" width="100" height="80"/><audio id="a" src="a.wav" duration="10"/><text id="txt" text="Hello" font="Inter" weight="700" size="32" width="200" height="80" align="center" color="#FF0000FF"/></assets>{before}<composition>{nodes}</composition>{after}</scene>"##
    )
}
fn run(xml: &str) -> (Imported, Report) {
    let (p, o, r) = import(xml.as_bytes(), &ImportOptions::default()).unwrap();
    assert!(o.is_empty());
    (p, r)
}
fn seq(p: &Imported) -> &Sequence {
    p.project.sequence(p.sequences[0]).unwrap()
}
fn clip<'a>(s: &'a Sequence, name: &str) -> &'a TrackItem {
    s.all_tracks().flat_map(|t| &t.items).find(|c| c.name == name).unwrap()
}
fn val(c: &TrackItem, name: &str) -> ParamValue {
    c.graphic_layers().next().unwrap().param(name).unwrap().value.clone()
}
#[test]
fn scene_s6_graphics() {
    let (p, _) = run(&scene(
        r#"<layer id="t" asset="txt" end="2"/><shape id="r" shape="rect" width="100" height="80" end="2"/><shape id="e" shape="ellipse" width="100" height="80" end="2"/><shape id="s" shape="star" width="100" height="100" points="5" end="2"/><shape id="p" shape="path" width="100" height="100" path="M0,0 L100,0 L0,100 Z" end="2"/>"#,
        "",
    ));
    let s = seq(&p);
    assert_eq!(val(clip(s, "t"), "text"), ParamValue::Text("Hello".into()));
    assert_eq!(val(clip(s, "t"), "size"), ParamValue::Float(32.));
    assert_eq!(val(clip(s, "t"), "font_style"), ParamValue::Text("Bold".into()));
    assert_eq!(val(clip(s, "t"), "align"), ParamValue::Choice(1));
    assert_eq!(val(clip(s, "t"), "fill_color"), ParamValue::Color([1., 0., 0., 1.]));
    for (id, shape) in [("r", 0), ("e", 1), ("s", 3), ("p", 3)] {
        assert_eq!(val(clip(s, id), "shape"), ParamValue::Choice(shape));
    }
    // Rect local centre is (50,40), so scene's top-left anchor becomes (-50,-40).
    assert_eq!(val(clip(s, "r"), "anchor"), ParamValue::Vec2(Vec2::new(-50., -40.)));
    let ParamValue::Curve(points) = val(clip(s, "s"), "points") else { panic!() };
    assert_eq!(points.len(), 10);
    assert_eq!(points[0], [50., 0.]);
    assert_eq!(val(clip(s, "p"), "points"), ParamValue::Curve(vec![[0., 0.], [100., 0.], [0., 100.]]));
}
#[test]
fn scene_s6_isolated_group() {
    let (p, _) = run(&scene(r#"<group id="g" isolate="true" start="2" end="6" opacity="0.5"><layer id="c" asset="i" start="2" end="4"/></group>"#, ""));
    let c = clip(seq(&p), "g");
    assert_eq!((c.start, c.duration), (sec(2), sec(4)));
    let nested = p.project.sequence(c.item).unwrap();
    assert_eq!((clip(nested, "c").start, clip(nested, "c").duration), (sec(0), sec(2)));
    assert_eq!(clip(nested, "c").effect("opacity").unwrap().param("opacity").unwrap().value.as_f64(), Some(100.));
    assert_eq!(c.effect("opacity").unwrap().param("opacity").unwrap().value.as_f64(), Some(50.));
}
#[test]
fn scene_s6_retimed_group() {
    let (p, _) = run(&scene(r#"<group id="g" start="2" end="6" timeScale="2"><layer id="c" asset="i" start="2" end="6"/></group>"#, ""));
    // Child clock: comp=1+local/2. Child [2,6) is comp [2,4); nest plays [0,8) at 2x.
    let c = clip(seq(&p), "g");
    assert_eq!((c.start, c.duration, c.speed), (sec(2), sec(4), 2.));
    let nested = p.project.sequence(c.item).unwrap();
    let child = clip(nested, "c");
    assert_eq!((child.start, child.duration, child.speed), (sec(0), sec(4), 1.));
}
#[test]
fn scene_s6_plain_group_sequence() {
    let (p, _) = run(&scene(
        r#"<group id="g" x="20" opacity="0.5"><shape id="r" shape="rect" width="10" height="10" x="10" opacity="0.5" end="2"/></group><sequence id="q" timeGap="1"><layer id="a1" asset="i" end="2"/><layer id="a2" asset="i" end="2"/></sequence>"#,
        "",
    ));
    assert_eq!(val(clip(seq(&p), "r"), "position"), ParamValue::Vec2(Vec2::new(30., 0.)));
    assert_eq!(val(clip(seq(&p), "r"), "opacity"), ParamValue::Float(25.));
    assert_eq!(clip(seq(&p), "a2").start, sec(3));
}
#[test]
fn scene_s6_one_sided_transition() {
    let (p, r) =
        run(&scene(r#"<layer id="c" asset="i" start="2" end="4"/><transition to="c" type="crossfade" duration="1" alignment="start" curve="linear"/>"#, ""));
    let trs: Vec<_> = seq(&p).video_tracks.iter().flat_map(|t| &t.transitions).collect();
    assert_eq!(trs.len(), 1, "{r}");
    assert_eq!((trs[0].start, trs[0].duration, trs[0].from), (sec(2), sec(1), None));
    assert_eq!(trs[0].effect.effect, "cross_dissolve");
}
#[test]
fn scene_s6_cross_track_transition() {
    let (p, r) = run(&scene(
        r#"<layer id="c1" asset="i" end="4"/><layer id="between" asset="i" start="0" end="8"/><layer id="c2" asset="i" start="4" end="8"/><transition from="c1" to="c2" type="crossfade" duration="2" curve="linear"/>"#,
        "",
    ));
    // The middle-painted layer overlaps both: A must be below it and B above it.
    assert!(seq(&p).video_tracks.iter().all(|t| t.transitions.is_empty()));
    assert!(r.mentions("different tracks"), "{r}");
}
#[test]
fn scene_s6_metadata_audio() {
    let extra = r##"<markers><marker id="m" time="2" duration="1" label="Chapter" kind="chapter" color="#568BF4"/></markers><audioMix><audioTrack id="voice" asset="a" start="1" clipIn="2" clipOut="5" volume="0.5" pan="-0.25"/></audioMix><captions><captionTrack id="cc" language="en" label="Speech"><cue start="1" end="3" text="Hello" speaker="Ada"/></captionTrack></captions>"##;
    let (p, _) = run(&scene("", extra));
    let s = seq(&p);
    assert_eq!(s.markers.len(), 1);
    let m = &s.markers[0];
    assert_eq!((&*m.name, m.start, m.duration), ("Chapter", sec(2), sec(1)));
    assert_eq!(s.caption_tracks[0].captions[0].duration, sec(2));
    assert_eq!(s.caption_tracks[0].captions[0].speaker.as_deref(), Some("Ada"));
    let a = s.audio_tracks.iter().find(|t| t.name == "voice").unwrap();
    assert!((a.volume_db + 6.020599913).abs() < 1e-8);
    assert_eq!(a.pan, -25.);
    assert_eq!((a.items[0].start, a.items[0].duration, a.items[0].source_in), (sec(1), sec(3), sec(2)));
}
#[test]
fn scene_s6_roundtrip() {
    let (p, _) = run(&scene(
        r#"<shape id="r" shape="rect" width="100" height="80" end="2"/><layer id="t" asset="txt" start="3" end="5"/><transition to="t" type="crossfade" duration="1" alignment="start" curve="linear"/>"#,
        r#"<markers><marker time="2" label="M" duration="1"/></markers><captions><captionTrack id="cc" language="en"><cue start="1" end="2" text="Caption"/></captionTrack></captions>"#,
    ).replace("weight=\"700\"", "weight=\"400\""));
    let mut report = Report::default();
    let xml = crate::scene::export::export(&p.project, p.sequences[0], &Default::default(), &mut report).unwrap();
    let (q, _) = run(&xml);
    let layers: Vec<_> = q
        .project
        .items
        .values()
        .filter_map(|i| i.as_sequence())
        .flat_map(|s| &s.video_tracks)
        .flat_map(|t| &t.items)
        .flat_map(|c| c.graphic_layers())
        .collect();
    assert_eq!(layers.len(), 2);
    assert!(layers.iter().any(|e| e.effect == graphic::TEXT_LAYER));
    for name in ["r", "t"] {
        let original = clip(seq(&p), name).graphic_layers().next().unwrap();
        let imported = layers.iter().find(|e| e.param("name").unwrap().value == ParamValue::Text(name.into())).unwrap();
        assert_eq!(imported.params, original.params, "graphic {name}");
    }
    let tr = seq(&q).video_tracks.iter().flat_map(|t| &t.transitions).next().unwrap();
    assert_eq!((tr.start, tr.duration, tr.effect.effect.as_str()), (sec(3), sec(1), "cross_dissolve"));
    assert_eq!(seq(&q).markers[0].start, sec(2));
    assert_eq!(seq(&q).caption_tracks[0].captions[0].duration, sec(1));
    assert_eq!(seq(&q).video_tracks.iter().map(|t| t.transitions.len()).sum::<usize>(), 1);
}
#[test]
fn scene_s6_deep_groups_no_panic() {
    for depth in [1, 32, 100, 127, 128, 129] {
        let nodes =
            format!("{}<shape id=\"s\" shape=\"rect\" width=\"2\" height=\"2\"/>{}", "<group isolate=\"true\">".repeat(depth), "</group>".repeat(depth));
        assert!(std::panic::catch_unwind(|| import(scene(&nodes, "").as_bytes(), &ImportOptions::default())).is_ok());
    }
}
proptest! { #![proptest_config(ProptestConfig::with_cases(32))]
 #[test] fn scene_s6_random_no_panic(values in prop::collection::vec((0u8..5,0u16..100,0u16..100),0..20)) {
 let mut nodes=String::new();for (i,(kind,a,b)) in values.iter().enumerate(){let shape=["rect","ellipse","star","polygon","path"][usize::from(*kind)];nodes.push_str(&format!(r#"<group isolate="true" timeScale="{}"><shape id="s{i}" shape="{shape}" width="10" height="10" path="M0 0L10 0L0 10Z" start="{a}" end="{b}"/></group>"#,a));}
 prop_assert!(std::panic::catch_unwind(||import(scene(&nodes,"").as_bytes(),&ImportOptions{lenient:true,..Default::default()})).is_ok(), "import panicked");
 }
}

#[test]
fn scene_s6_span_text_and_nested_paint_order() {
    let xml=scene(r#"<group id="outer" isolate="true"><group id="inner" isolate="true"><layer id="text" asset="txt"/></group><shape id="above" shape="rect" width="10" height="10"/></group>"#,"").replace("text=\"Hello\"", "").replace("color=\"#FF0000FF\"/>","color=\"#FF0000FF\"><span>One </span><span>two</span></text>");
    let (p, _) = run(&xml);
    let outer = p.project.sequence(clip(seq(&p), "outer").item).unwrap();
    // Nested subtree is painted first, then its following sibling is above it.
    let names: Vec<_> = outer.video_tracks.iter().flat_map(|t| &t.items).filter(|c| c.enabled).map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["inner", "above"]);
    let inner = p.project.sequence(clip(outer, "inner").item).unwrap();
    assert_eq!(val(clip(inner, "text"), "text"), ParamValue::Text("One two".into()));
}
#[test]
fn scene_s6_path_subset_and_reports() {
    let (p, r) = run(&scene(
        r#"<shape id="p" shape="path" width="10" height="10" path="m1 2 h3 v4 l-3 0z"/><shape id="curve" shape="path" width="10" height="10" path="M0 0 C0 1 1 1 1 0Z"/>"#,
        "",
    ));
    // Relative commands yield (1,2),(4,2),(4,6),(1,6).
    assert_eq!(val(clip(seq(&p), "p"), "points"), ParamValue::Curve(vec![[1., 2.], [4., 2.], [4., 6.], [1., 6.]]));
    assert!(r.mentions("Unmapped SVG path"));
}
#[test]
fn scene_s6_audio_reverse_trim() {
    let (p, _) = run(&scene("", r#"<audioMix><audioTrack id="rev" asset="a" start="11" clipIn="2" clipOut="6" reverse="true"/></audioMix>"#));
    // Last project second takes the first second of reverse playback, source [5,6).
    let c = clip(seq(&p), "rev");
    assert_eq!((c.start, c.duration, c.source_in, c.reverse), (sec(11), sec(1), sec(5), true));
}
#[test]
fn scene_s6_span_text_content() {
    let xml = scene(r#"<layer id="t" asset="txt"/>"#, "")
        .replace("text=\"Hello\"", "")
        .replace("color=\"#FF0000FF\"/>", "color=\"#FF0000FF\"><span>One </span><span>two</span></text>");
    let (p, _) = run(&xml);
    assert_eq!(val(clip(seq(&p), "t"), "text"), ParamValue::Text("One two".into()));
}
#[test]
fn scene_s6_drop_nested_overlays() {
    let xml = scene(r#"<group id="g" timeScale="2"><group id="fx" threeD="true"/></group>"#, "");
    let (mut p, overlays, _) = import(xml.as_bytes(), &Default::default()).unwrap();
    assert_eq!(overlays.len(), 1);
    drop_overlays(&mut p, &overlays).unwrap();
    for s in p.project.items.values().filter_map(|i| i.as_sequence()) {
        for c in s.all_tracks().flat_map(|t| &t.items) {
            assert!(p.project.item(c.item).is_some(), "dangling clip {}", c.name);
        }
    }
}
#[test]
fn scene_s6_audio_automation_report() {
    let (_, r) = run(&scene(
        "",
        r#"<audioMix><audioTrack id="music" asset="a"><animate property="volume"><key time="0" value="0"/><key time="1" value="1"/></animate></audioTrack></audioMix>"#,
    ));
    assert!(r.mentions("audio automation"), "{r}");
}
#[test]
fn scene_s6_group_overlay_not_duplicated() {
    let xml = scene(r#"<group id="g" isolate="true"><expression property="opacity">0.5</expression><layer id="c" asset="i"/></group>"#, "");
    let (p, o, _) = import(xml.as_bytes(), &Default::default()).unwrap();
    assert_eq!(o.len(), 1);
    assert_eq!(seq(&p).video_tracks.iter().flat_map(|t| &t.items).filter(|c| c.name == "g").count(), 1);
}
#[test]
fn scene_s6_metadata_count_limit() {
    let xml = scene("", &format!("<markers>{}</markers>", "<marker time=\"0\"/>".repeat(100_001)));
    assert!(import(xml.as_bytes(), &Default::default()).is_err());
}
proptest! { #![proptest_config(ProptestConfig::with_cases(32))]
 #[test] fn scene_s6_mixed_valid_no_panic(kinds in prop::collection::vec(0u8..5,1..12)) {
 let mut nodes=String::new();for (i,kind) in kinds.iter().enumerate(){match kind{
 0=>nodes.push_str(&format!(r#"<layer id="n{i}" asset="txt" start="1" end="3"/>"#)),
 1=>nodes.push_str(&format!(r#"<group id="g{i}" isolate="true" timeScale="2"><shape id="n{i}" shape="star" width="20" height="20" end="2"/></group>"#)),
 2=>nodes.push_str(&format!(r#"<sequence id="g{i}" timeGap="1"><layer id="n{i}" asset="i" end="1"/><shape id="s{i}" shape="polygon" width="20" height="20" end="1"/></sequence>"#)),
 _=>nodes.push_str(&format!(r#"<shape id="n{i}" shape="ellipse" width="20" height="20" start="1" end="3"/><transition to="n{i}" type="crossfade" duration="0.5" alignment="start"/>"#))}}
 let xml=scene(&nodes,r#"<markers><marker time="1"/></markers><audioMix><audioTrack id="music" asset="a" volume="0.5" pan="0.5"/></audioMix><captions><captionTrack id="cc" language="en"><cue start="0" end="1" text="Caption"/></captionTrack></captions>"#);
 let result=std::panic::catch_unwind(||import(xml.as_bytes(),&Default::default()));prop_assert!(result.is_ok(),"panic");prop_assert!(result.unwrap().is_ok(),"valid mixed scene rejected");
 }
}
#[test]
fn scene_s6_transition_table_default_color() {
    for (kind, effect) in [("crossfade", "cross_dissolve"), ("additive-dissolve", "additive_dissolve"), ("dip-to-color", "dip_to_color")] {
        let (p, r) =
            run(&scene(&format!(r#"<layer id="c" asset="i" start="2" end="4"/><transition to="c" type="{kind}" duration="1" alignment="start"/>"#), ""));
        let tr = seq(&p).video_tracks.iter().flat_map(|t| &t.transitions).next().unwrap();
        assert_eq!(tr.effect.effect, effect);
        assert!(r.mentions("curve"));
        // transitionType@color defaults to opaque black, independently of FilmCraft's effect preset.
        if kind == "dip-to-color" {
            assert_eq!(tr.effect.param("color").unwrap().value, ParamValue::Color([0., 0., 0., 1.]));
        }
    }
}
#[test]
fn scene_s6_missing_transition_endpoint_report() {
    let (_, r) = run(&scene(
        r#"<shape id="p" shape="path" width="10" height="10" path="M0 0 C0 1 1 1 1 0Z"/><transition to="p" type="crossfade" duration="1" alignment="start"/>"#,
        "",
    ));
    assert!(r.mentions("transition"), "{r}");
}

#[test]
fn scene_fix1_dissolve_occupancy_and_roundtrip() {
    // Nominal A [0,4), B [4,8); centered 2s window [3,5). B starts at source 2s.
    let (p, _) = run(&scene(
        r#"<layer id="a1" asset="i" end="4" clipIn="2" clipOut="6"/><layer id="b1" asset="i" start="4" end="8" clipIn="2" clipOut="6"/><transition from="a1" to="b1" type="crossfade" duration="2" curve="linear"/>"#,
        "",
    ));
    assert_eq!((clip(seq(&p), "a1").end(), clip(seq(&p), "b1").start, clip(seq(&p), "b1").source_in), (sec(4), sec(4), sec(2)));
    let track = seq(&p).video_tracks.iter().find(|t| !t.transitions.is_empty()).unwrap();
    assert_eq!(track.items.iter().filter(|c| c.enabled).count(), 2);
    assert_eq!((track.transitions[0].start, track.transitions[0].duration), (sec(3), sec(2)));
    let xml = crate::scene::export::export(&p.project, p.sequences[0], &Default::default(), &mut Report::default()).unwrap();
    let (q, _) = run(&xml);
    let trs: Vec<_> = seq(&q).video_tracks.iter().flat_map(|t| &t.transitions).collect();
    assert_eq!(trs.len(), 1);
    assert_eq!((trs[0].start, trs[0].duration), (sec(3), sec(2)));
}
#[test]
fn scene_fix1_joint_track_and_one_sided_edges() {
    // Underlay overlaps B only; both A and B can jointly sit above it.
    let (p, _) = run(&scene(
        r#"<layer id="under" asset="i" start="4" end="8"/><layer id="a1" asset="i" end="4"/><layer id="b1" asset="i" start="4" end="8"/><transition from="a1" to="b1" type="crossfade" duration="2" curve="linear"/>"#,
        "",
    ));
    assert_eq!(seq(&p).video_tracks.iter().map(|t| t.transitions.len()).sum::<usize>(), 1);
    for (side, alignment) in [("to", "center"), ("from", "center"), ("to", "end"), ("from", "start")] {
        let (p, _) = run(&scene(
            &format!(
                r#"<layer id="c" asset="i" start="2" end="4" clipIn="2" clipOut="4"/><transition {side}="c" type="crossfade" duration="1" alignment="{alignment}" curve="linear"/>"#
            ),
            "",
        ));
        assert_eq!((clip(seq(&p), "c").start, clip(seq(&p), "c").end(), clip(seq(&p), "c").source_in), (sec(2), sec(4), sec(2)));
    }
}
#[test]
fn scene_fix2_overlay_effective_opacity() {
    let xml = scene(r#"<group id="g" isolate="true" opacity="0.5"><group id="fx" threeD="true"/></group>"#, "");
    let (p, overlays, _) = import(xml.as_bytes(), &Default::default()).unwrap();
    // Preserve the complete isolated composite: baked alpha .5, native carrier alpha 1.
    assert_eq!(overlays.len(), 1);
    let c = clip(seq(&p), "g");
    assert_eq!(c.item, overlays[0].item);
    assert_eq!(c.effect("opacity").unwrap().param("opacity").unwrap().value.as_f64(), Some(100.));
    let xml = std::str::from_utf8(&overlays[0].document).unwrap();
    let doc = read::read(xml, &Default::default()).unwrap();
    let r = resolve::resolve(&doc, &Default::default()).unwrap();
    assert_eq!(r.nodes.iter().find(|n| n.id == "fx").unwrap().opacity, 0.5);
}
#[test]
fn scene_fix3_clock_only_opacity() {
    let (p, _) = run(&scene(
        r#"<group id="g" timeScale="2" isolate="false" opacity="0.5"><shape id="a1" shape="rect" width="10" height="10"/><shape id="b1" shape="rect" width="10" height="10"/></group>"#,
        "",
    ));
    let c = clip(seq(&p), "g");
    let s = p.project.sequence(c.item).unwrap();
    // Two half-alpha opaque shapes cover 1-(1-.5)^2=.75, without a boundary fade.
    assert_eq!(val(clip(s, "a1"), "opacity"), ParamValue::Float(50.));
    assert_eq!(val(clip(s, "b1"), "opacity"), ParamValue::Float(50.));
    assert_eq!(c.effect("opacity").unwrap().param("opacity").unwrap().value.as_f64(), Some(100.));
}
#[test]
fn scene_fix4_nested_audio_exact_edges() {
    for end in ["0.05", "0.13"] {
        let (p, _) = run(&scene(
            &format!(r#"<group id="g" isolate="true" start="0.03" end="{end}"><layer id="sound" asset="a" start="0.03" end="{end}"/></group>"#),
            "",
        ));
        let audio = seq(&p).audio_tracks.iter().flat_map(|t| &t.items).find(|c| c.name == "g").unwrap();
        // 30ms is exact audio time even though video rounds to 40ms; 50ms also rounds to 40ms.
        assert_eq!(audio.start, Tick(TICKS_PER_SECOND * 3 / 100));
        assert_eq!(audio.end(), if end == "0.05" { Tick(TICKS_PER_SECOND / 20) } else { Tick(TICKS_PER_SECOND * 13 / 100) });
        let nested = p.project.sequence(audio.item).unwrap();
        let child = clip(nested, "sound");
        assert_eq!(audio.source_in, child.start);
        assert_eq!(child.duration, audio.duration);
    }
}
#[test]
fn scene_fix5_deep_content_reachable() {
    let nodes = format!(
        "{}<shape id=\"s\" shape=\"rect\" width=\"2\" height=\"2\"/>{}",
        (0..10).map(|i| format!("<group id=\"g{i}\" isolate=\"true\">")).collect::<String>(),
        "</group>".repeat(10)
    );
    let (p, overlays, report) = import(scene(&nodes, "").as_bytes(), &Default::default()).unwrap();
    let mut pending = vec![(p.sequences[0], 0)];
    let mut leaves = 0;
    while let Some((id, depth)) = pending.pop() {
        assert!(depth <= 8, "native depth {depth} exceeds renderer limit");
        for c in p.project.sequence(id).unwrap().video_tracks.iter().flat_map(|t| &t.items).filter(|c| c.enabled) {
            if p.project.sequence(c.item).is_some() {
                pending.push((c.item, depth + 1));
            } else if c.graphic_layers().next().is_some() || overlays.iter().any(|o| o.item == c.item) {
                leaves += 1;
            }
        }
    }
    assert_eq!(leaves, 1);
    assert!(report.mentions("depth"));
}
#[test]
fn scene_fix6_audio_loops_adjacent_ticks() {
    let xml = scene("", r#"<audioMix><audioTrack id="looped" asset="a" start="100000.1" clipIn="0.1" clipOut="0.2" speed="1" loop="2"/></audioMix>"#)
        .replace("duration=\"12\"", "duration=\"100001\"");
    let (p, _) = run(&xml);
    let t = seq(&p).audio_tracks.iter().find(|t| t.name == "looped").unwrap();
    // Three plays of exactly 1/10 second share integer boundaries.
    assert_eq!(t.items.len(), 3);
    for pair in t.items.windows(2) {
        assert_eq!(pair[0].end(), pair[1].start);
    }
    for c in &t.items {
        assert_eq!(c.duration, Tick(TICKS_PER_SECOND / 10));
    }
}
#[test]
fn scene_fix7_drop_overlay_transitions() {
    let xml = scene(
        r#"<group id="g" timeScale="2"><group id="fx" threeD="true"/><transition to="fx" type="crossfade" duration="1" alignment="start" curve="linear"/></group>"#,
        "",
    );
    let (mut p, overlays, _) = import(xml.as_bytes(), &Default::default()).unwrap();
    assert_eq!(p.project.items.values().filter_map(|i| i.as_sequence()).flat_map(|s| &s.video_tracks).map(|t| t.transitions.len()).sum::<usize>(), 1);
    drop_overlays(&mut p, &overlays).unwrap();
    for t in p.project.items.values().filter_map(|i| i.as_sequence()).flat_map(|s| s.all_tracks()) {
        for tr in &t.transitions {
            for id in tr.from.into_iter().chain(tr.to) {
                assert!(t.items.iter().any(|c| c.id == id), "dangling transition endpoint");
            }
        }
    }
}
#[test]
fn scene_fix8_font_family_list() {
    let (p, report) = run(&scene(r#"<layer id="t" asset="txt"/>"#, "").replace("font=\"Inter\"", "font=\"'Arial', Inter\""));
    // The pure importer has no installed-font database: preserve first preference, report lost fallback chain.
    assert_eq!(val(clip(seq(&p), "t"), "font"), ParamValue::Text("Arial".into()));
    assert!(report.mentions("font fallback"));
}
#[test]
fn scene_fix9_span_comment_text() {
    let xml = scene(r#"<layer id="t" asset="txt"/>"#, "")
        .replace("text=\"Hello\"", "")
        .replace("color=\"#FF0000FF\"/>", "color=\"#FF0000FF\"><span>One<!-- editorial comment --> two</span></text>");
    let (p, _) = run(&xml);
    assert_eq!(val(clip(seq(&p), "t"), "text"), ParamValue::Text("One two".into()));
}

#[test]
fn scene_fix6_audio_rational_duration() {
    // 2,400,000 source seconds / 3 = exactly 800,000 timeline seconds.
    let xml =
        scene("", r#"<audioMix><audioTrack id="third" asset="a" clipOut="2400000" speed="3"/></audioMix>"#).replace("duration=\"12\"", "duration=\"1000000\"");
    let (p, _) = run(&xml);
    assert_eq!(clip(seq(&p), "third").duration, sec(800_000));
}
#[test]
fn scene_fix1_conflict_does_not_break_other_pair() {
    // A/X/B cannot share a track. Independently C/D can both go above Y.
    let (p, report) = run(&scene(
        r#"<layer id="a1" asset="i" end="2"/><layer id="x" asset="i" end="4"/><layer id="b1" asset="i" start="2" end="4"/><layer id="y" asset="i" start="6" end="8"/><layer id="c" asset="i" start="4" end="6"/><layer id="d" asset="i" start="6" end="8"/><transition from="a1" to="b1" type="crossfade" duration="1" curve="linear"/><transition from="c" to="d" type="crossfade" duration="1" curve="linear"/>"#,
        "",
    ));
    let transitions: Vec<_> = seq(&p).video_tracks.iter().flat_map(|t| &t.transitions).collect();
    assert_eq!(transitions.len(), 1, "{report}");
    // At 25fps, half-even snaps 137.5 -> 138 and 162.5 -> 162: 24 frames.
    assert_eq!((transitions[0].start, transitions[0].duration), (Tick(TICKS_PER_SECOND * 138 / 25), Tick(TICKS_PER_SECOND * 24 / 25)));
    assert!(report.mentions("different tracks"));
}
#[test]
fn scene_fix1_nest_handle_source_origin() {
    // Incoming group starts at 4 but its source includes a 3s pre-roll. Child starts
    // at composition 3, so source 0 is pre-roll and the carrier's in-point is 1s.
    let (p, _) = run(&scene(
        r#"<layer id="a1" asset="i" end="4"/><group id="g" isolate="true" start="4" end="8"><layer id="child" asset="i" start="3" end="8"/></group><transition from="a1" to="g" type="crossfade" duration="2" curve="linear"/>"#,
        "",
    ));
    let c = clip(seq(&p), "g");
    assert_eq!((c.start, c.end(), c.source_in), (sec(4), sec(8), sec(1)));
    let nested = p.project.sequence(c.item).unwrap();
    assert_eq!((clip(nested, "child").start, clip(nested, "child").duration), (sec(0), sec(5)));
    assert_eq!(seq(&p).video_tracks.iter().map(|t| t.transitions.len()).sum::<usize>(), 1);
}
#[test]
fn scene_fix3_clock_nested_in_isolation() {
    // Outer .5 composite, inner clock group .5 inherited per child: .5 * .75 = .375.
    let (p, _) = run(&scene(
        r#"<group id="outer" isolate="true" opacity="0.5"><group id="clock" timeScale="2" opacity="0.5"><shape id="a1" shape="rect" width="10" height="10"/><shape id="b1" shape="rect" width="10" height="10"/></group></group>"#,
        "",
    ));
    let outer = clip(seq(&p), "outer");
    let clock = clip(p.project.sequence(outer.item).unwrap(), "clock");
    let child = clip(p.project.sequence(clock.item).unwrap(), "a1");
    assert_eq!(outer.effect("opacity").unwrap().param("opacity").unwrap().value.as_f64(), Some(50.));
    assert_eq!(clock.effect("opacity").unwrap().param("opacity").unwrap().value.as_f64(), Some(100.));
    assert_eq!(val(child, "opacity"), ParamValue::Float(50.));
}
