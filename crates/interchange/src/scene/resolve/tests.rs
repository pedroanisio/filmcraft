use super::*;
use crate::scene::{read, validate};

fn xml(nodes: &str, markers: &str) -> String {
    format!(
        r#"<scene version="1.7"><project width="1920" height="1080" fps="30000/1001" duration="30"/><assets><video id="v" src="v.mp4" width="1920" height="1080" fps="30" duration="30"/><image id="img" src="i.png" width="640" height="480"/></assets>{markers}<composition>{nodes}</composition></scene>"#
    )
}
fn checked(text: &str, limits: ResolveLimits) -> Result<Resolved> {
    let doc = read::read(text, &Default::default()).unwrap();
    let errors: Vec<_> = validate::validate(&doc.doc).into_iter().filter(|d| d.is_error()).collect();
    assert!(errors.is_empty(), "fixture must validate: {errors:?}");
    resolve(&doc, &limits)
}
fn run(nodes: &str) -> Resolved {
    checked(&xml(nodes, ""), ResolveLimits::default()).unwrap()
}
fn node<'a>(r: &'a Resolved, id: &str) -> &'a ResolvedNode {
    r.nodes.iter().find(|n| n.id == id).unwrap()
}
fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-8, "{a} != {b}");
}
fn segments<'a>(r: &'a Resolved, id: &str) -> &'a [SourceSegment] {
    match node(r, id).source.as_ref().unwrap() {
        SourceMap::Segments(s) => s,
        _ => panic!("expected segments"),
    }
}

#[test]
fn scene_resolve_source_spans_rates_and_holds() {
    let r = run(r#"
      <layer id="clip" asset="v" start="1" clipIn="2" clipOut="6"/>
      <layer id="fast" asset="v" clipIn="2" clipOut="6" speed="2"/>
      <layer id="slow" asset="v" clipIn="2" clipOut="6" speed="0.5"/>
      <layer id="cancel" asset="v" clipIn="2" clipOut="6" speed="-2" reverse="true"/>
      <layer id="back" asset="v" clipIn="2" clipOut="6" reverse="true"/>
      <layer id="hold" asset="v" end="4" clipIn="2" clipOut="6" speed="0"/>
      <layer id="stretch" asset="v" clipIn="2" clipOut="6" speed="2" timeStretch="4"/>
      <layer id="freeze" asset="v" end="8" clipIn="2" clipOut="6" speed="2" freezeAt="1"/>
      <layer id="unknown" asset="img" end="5"/>
      <layer id="loops" asset="v" clipIn="2" clipOut="6" loop="2"/>
    "#);
    // Source span is 6-2=4 s; duration = span / |speed/timeStretch|.
    for (id, end, src, rate) in [
        ("clip", 5., 2., 1.),
        ("fast", 2., 2., 2.),
        ("slow", 8., 2., 0.5),
        ("cancel", 2., 2., 2.),
        ("back", 4., 6., -1.),
        ("hold", 4., 2., 0.),
        ("stretch", 8., 2., 0.5),
        ("freeze", 8., 4., 0.),
    ] {
        near(node(&r, id).interval.end, end);
        near(segments(&r, id)[0].src_start, src);
        near(segments(&r, id)[0].rate, rate);
    }
    // loop=2 is three plays, each 4 seconds, resetting to source 2.
    assert_eq!(
        segments(&r, "loops"),
        [
            SourceSegment { comp_start: 0., comp_end: 4., src_start: 2., rate: 1. },
            SourceSegment { comp_start: 4., comp_end: 8., src_start: 2., rate: 1. },
            SourceSegment { comp_start: 8., comp_end: 12., src_start: 2., rate: 1. },
        ]
    );
    assert!(r.notes.iter().any(|n| n.code == "MEDIA_DURATION_INFERRED"));
}

#[test]
fn scene_resolve_remap_supersedes_playback() {
    let r = run(
        r#"<layer id="r" asset="v" start="3" end="9" clipIn="20"><timeRemap><key time="0" value="7"/><key time="2" value="4" interpolation="hold"/></timeRemap></layer>"#,
    );
    // Remap keys are layer-local: composition 3+0 and 3+2; values are absolute source seconds.
    let SourceMap::Remap { keys, .. } = node(&r, "r").source.as_ref().unwrap() else { panic!() };
    assert_eq!(keys.iter().map(|k| (k.comp_time, k.source_time, k.interpolation.as_str())).collect::<Vec<_>>(), [(3., 7., "linear"), (5., 4., "hold")]);
}

#[test]
fn scene_resolve_group_clock_inheritance_and_key_bases() {
    let r = run(
        r#"<group id="g" start="2" end="14" timeOffset="1" timeScale="2"><layer id="a" asset="v" start="4" end="8" clipIn="1" clipOut="9"><animate property="opacity"><key time="4" value="0"/><key time="8" value="1"/></animate></layer><layer id="b" asset="img" start="6"/></group>"#,
    );
    // Child time u = 2 + (t-2-1)*2 = 2t-4; inverse t = 2+u/2.
    assert_eq!(node(&r, "a").interval, Interval { start: 4., end: 6. });
    near(segments(&r, "a")[0].rate, 2.);
    assert_eq!(node(&r, "b").interval, Interval { start: 5., end: 14. });
    let a = node(&r, "a").index;
    assert_eq!(r.key_time(a, TimeBase::Composition, 6.), Some(5.));
    assert_eq!(r.key_time(a, TimeBase::Local, 1.), Some(4.5));
    assert_eq!(r.key_time(a, TimeBase::Normalized, 0.5), Some(5.));
    assert_eq!(node(&r, "a").animations[0].keys.iter().map(|k| k.1).collect::<Vec<_>>(), [4., 6.]);
    assert_eq!(r.project.fps.num, 30000);
    assert_eq!(r.project.fps.den, 1001);
    near(r.project.duration, 30.);
}

#[test]
fn scene_resolve_sequence_markers_and_defaults() {
    let text = xml(
        r#"<sequence id="s" timeGap="1" transition="crossfade" transitionDuration="2"><layer id="a" asset="v" clipOut="2"/><layer id="b" asset="v" start="0.5" clipOut="3"/><layer id="c" asset="v" startMarker="m" start="1" endMarker="e" end="-1"/></sequence>"#,
        r#"<markers><marker id="m" time="10"/><marker id="e" time="15"/></markers>"#,
    );
    let r = checked(&text, Default::default()).unwrap();
    // A ends at 2; B starts 2+1+0.5=3.5 and ends 6.5. Marker C stays [11,14).
    assert_eq!(node(&r, "b").interval, Interval { start: 3.5, end: 6.5 });
    assert_eq!(node(&r, "c").interval, Interval { start: 11., end: 14. });
    assert_eq!(r.transitions.len(), 2);
    assert_eq!(r.transitions[0].window, Interval { start: 1., end: 3. });
    assert_eq!(node(&r, "a").attributes.get("blend").map(String::as_str), Some("normal"));
}

#[test]
fn scene_resolve_layout_units_anchor_parent_matrix_and_z() {
    let r = run(
        r#"<group id="g" width="400" height="200" x="10" rotation="90" scaleX="2" scaleY="3" opacity="0.5"><shape id="front" shape="rect" width="50%" height="50%" x="50%" y="25%" opacity="0.5" z="2"/><shape id="back" shape="rect" width="10vw" height="10vh" x="5" y="7" z="-1"/><layer id="fit" asset="img" fit="contain" boxWidth="200" boxHeight="100" anchorX="100" anchorY="50"/></group>"#,
    );
    let front = node(&r, "front");
    let l = front.layout.as_ref().unwrap();
    assert_eq!((l.width, l.height, l.transform.x, l.transform.y), (200., 100., 200., 50.));
    near(front.opacity, 0.25);
    let back = node(&r, "back").layout.as_ref().unwrap();
    assert_eq!((back.width, back.height), (192., 108.));
    // Parent T(10,0) R(90) S(2,3) maps child origin (5,7) to (-11,10).
    let m = back.world_matrix.unwrap();
    near(m.e, -11.);
    near(m.f, 10.);
    near(m.b, 2.);
    near(m.c, -3.);
    let fit = node(&r, "fit").layout.as_ref().unwrap();
    assert_eq!((fit.width, fit.height, fit.transform.anchor_x, fit.transform.anchor_y), (200., 100., 100., 50.));
    assert_eq!(r.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["g", "back", "fit", "front"]);
    assert_eq!(r.node(front.index).unwrap().id, "front");
}

#[test]
fn scene_resolve_transitions_and_extended_spans() {
    let r = run(
        r#"<layer id="a" asset="v" end="5"/><layer id="b" asset="v" start="5" end="10"/><transition type="crossfade" from="a" to="b" duration="2"/><transition type="wipe" to="a" duration="2" alignment="start"/><transition type="wipe" from="b" duration="2" alignment="end"/>"#,
    );
    assert_eq!(r.transitions.len(), 3);
    assert_eq!(r.transitions[0].window, Interval { start: 4., end: 6. });
    assert_eq!(r.transitions[0].curve, "ease-in-out");
    assert_eq!(r.transitions[1].window, Interval { start: 0., end: 2. });
    assert_eq!(r.transitions[1].from, None);
    assert_eq!(r.transitions[2].window, Interval { start: 8., end: 10. });
    assert_eq!(node(&r, "a").span, Interval { start: 0., end: 6. });
    assert_eq!(node(&r, "b").span, Interval { start: 4., end: 10. });
}

#[test]
fn scene_resolve_unmappable_roots_and_animated_parent() {
    let r = run(
        r#"<group id="unsupported" threeD="true"><layer id="nested" asset="v" threeD="true"/></group><group id="animated"><animate property="rotation"><key time="0" value="0"/><key time="1" value="90"/></animate><layer id="child" asset="v"><expression property="opacity">0.5</expression></layer></group>"#,
    );
    assert_eq!(r.unmappable.len(), 1);
    assert_eq!(r.unmappable[0].node, node(&r, "unsupported").index);
    assert!(node(&r, "child").unsupported.iter().any(|s| s == "expression"));
    assert!(node(&r, "child").layout.as_ref().unwrap().parent_transform_unresolved);
}

#[test]
fn scene_resolve_hostile_loop_cap() {
    let text = xml(r#"<layer id="a" asset="v" clipOut="1" loop="2"/>"#, "");
    assert!(checked(&text, ResolveLimits { max_segments: 2, ..Default::default() }).is_err());
}
#[test]
fn scene_resolve_hostile_parent_cycle() {
    let text = xml(r#"<layer id="a" asset="v" parent="b"/><layer id="b" asset="v" parent="a"/>"#, "");
    assert!(checked(&text, Default::default()).is_err());
}
#[test]
fn scene_resolve_hostile_depth() {
    let text = xml(r#"<group id="a"><group id="b"><layer id="c" asset="v"/></group></group>"#, "");
    assert!(checked(&text, ResolveLimits { max_depth: 1, ..Default::default() }).is_err());
}
#[test]
fn scene_resolve_hostile_overflow() {
    // Both factors are finite; their product must be rejected.
    let text = xml(r#"<group id="g" scaleX="1e308"><layer id="c" asset="v" scaleX="1e308"/></group>"#, "");
    assert!(checked(&text, Default::default()).is_err());
}
#[test]
fn scene_resolve_hostile_nonfinite() {
    for v in ["NaN", "INF", "-INF"] {
        let text = xml(&format!(r#"<layer id="a" asset="v" speed="{v}"/>"#), "");
        assert!(checked(&text, Default::default()).is_err());
        assert_eq!(crate::scene::values::seconds_to_tick(v.parse().unwrap_or(f64::NAN)), None);
    }
}

#[test]
fn scene_resolve_inherited_end_transition_uses_incoming_start() {
    let r = run(r#"<layer id="a" asset="img"/><layer id="b" asset="img" start="5" end="10"/><transition type="crossfade" from="a" to="b" duration="2"/>"#);
    // A has no intrinsic media duration and no explicit end. XSD transitionType uses B's start.
    assert_eq!(r.transitions[0].cut, 5.);
    assert_eq!(r.transitions[0].window, Interval { start: 4., end: 6. });
}

#[test]
fn scene_resolve_transition_source_handles() {
    let r = run(
        r#"<layer id="a" asset="v" clipIn="2" end="5"/><layer id="b" asset="v" clipIn="3" start="5" end="10"/><transition type="crossfade" from="a" to="b" duration="2"/>"#,
    );
    // Outgoing extends to comp 6/source 8. Incoming starts a second early at comp 4/source 2.
    assert_eq!(segments(&r, "a").last().unwrap().comp_end, 6.);
    assert_eq!(segments(&r, "b")[0].comp_start, 4.);
    assert_eq!(segments(&r, "b")[0].src_start, 2.);
}

#[test]
fn scene_resolve_rate_underflow_is_error() {
    // Nonzero speed divided by a finite stretch must not silently turn into a frame hold.
    let text = xml(r#"<layer id="a" asset="v" speed="1e-300" timeStretch="1e300"/>"#, "");
    assert!(checked(&text, Default::default()).is_err());
}

#[test]
fn scene_resolve_transition_handles_respect_container_window() {
    let r = run(
        r#"<group id="g" start="2" end="8" timeScale="2"><layer id="a" asset="v" start="2" end="6"/><layer id="b" asset="v" start="6" end="10"/><transition type="crossfade" from="a" to="b" duration="2"/><transition type="wipe" to="a" duration="4"/></group>"#,
    );
    // u=2+(t-2)*2: cut u=6 => t=4, duration 2/2=1. In-handle stays inside group [2,8).
    assert_eq!(r.transitions[0].window, Interval { start: 3.5, end: 4.5 });
    assert_eq!(node(&r, "a").span.start, 2.);
}

#[test]
fn scene_resolve_explicit_end_keeps_unlooped_source_clock_running() {
    let r = run(r#"<layer id="a" asset="v" clipIn="2" clipOut="4" end="5"/><layer id="b" asset="v" clipIn="2" clipOut="4" loop="1" end="5"/>"#);
    // sr-eval MediaClock::map only clamps at the final play when loop>0.
    // Explicit end overrides intrinsic duration: A reaches source 7, B holds source 4 after comp 4.
    let a = segments(&r, "a").last().unwrap();
    near(a.src_start + (a.comp_end - a.comp_start) * a.rate, 7.);
    let b = segments(&r, "b").last().unwrap();
    assert_eq!(*b, SourceSegment { comp_start: 4., comp_end: 5., src_start: 4., rate: 0. });
}

#[test]
fn scene_resolve_remap_ignores_rate_arithmetic_and_loop_budget() {
    let text = xml(
        r#"<layer id="a" asset="v" clipIn="20" speed="1e300" timeStretch="1e-300" loop="1000000" reverse="true"><timeRemap><key time="0" value="2"/><key time="1" value="4"/></timeRemap></layer>"#,
        "",
    );
    let doc = read::read(&text, &Default::default()).unwrap();
    // C15 prohibits explicitly combining playback controls with remap. In a lenient pipeline,
    // the documented remap precedence still applies, without evaluating ignored arithmetic.
    let errors: Vec<_> = validate::validate(&doc.doc).into_iter().filter(|d| d.is_error()).collect();
    assert!(!errors.is_empty());
    assert!(errors.iter().all(|d| d.code == "C15"));
    let r = resolve(&doc, &ResolveLimits { max_segments: 0, ..Default::default() }).unwrap();
    let SourceMap::Remap { keys, .. } = node(&r, "a").source.as_ref().unwrap() else { panic!() };
    assert_eq!(keys.iter().map(|k| k.source_time).collect::<Vec<_>>(), [2., 4.]);
}

#[test]
fn scene_resolve_static_parent_reference_overrides_containment_transform() {
    let r = run(
        r#"<group id="box" x="100" opacity="0.5"><layer id="child" asset="img" parent="target" x="3" y="4" opacity="0.5"/></group><group id="target" x="10" rotation="90" scaleX="2" scaleY="3" opacity="0.1"/>"#,
    );
    // Explicit transform parent replaces the containment transform: (3,4) -> (-2,6).
    // Opacity still follows XML containment: 0.5 * 0.5, not target's 0.1.
    let c = node(&r, "child");
    let m = c.layout.as_ref().unwrap().world_matrix.unwrap();
    near(m.e, -2.);
    near(m.f, 6.);
    near(c.opacity, 0.25);
    assert!(!c.unsupported.iter().any(|s| s == "parent"));
    assert!(!r.notes.iter().any(|n| n.node == Some(c.index) && n.code == "PARENT_TRANSFORM"));
}

#[test]
fn scene_resolve_group_transition_handles_reach_inheriting_children() {
    let r = run(
        r#"<group id="a" end="5"><layer id="child" asset="img"/></group><group id="b" start="5" end="10"/><transition type="crossfade" from="a" to="b" duration="2"/>"#,
    );
    // The transition extends A to 6; its child has no own end and stays with A's handle.
    assert_eq!(node(&r, "a").span, Interval { start: 0., end: 6. });
    assert_eq!(node(&r, "child").span, Interval { start: 0., end: 6. });
    assert_eq!(segments(&r, "child").last().unwrap().comp_end, 6.);
}
