use super::*;
use crate::{
    Error, Format,
    scene::{read, validate},
};
use filmcraft_project::{ItemKind, MediaRef, Sequence, TrackItem};
use filmcraft_time::{TICKS_PER_SECOND, Tick};

fn scene(nodes: &str) -> String {
    format!(
        r#"<scene version="1.7"><project width="640" height="480" fps="25" duration="12"/><assets><video id="v" src="v.mp4" width="640" height="480" fps="25" duration="10" hasAudio="true"/><image id="i" src="i.png" width="100" height="80"/></assets><composition>{nodes}</composition></scene>"#
    )
}
fn run(xml: &str) -> (Imported, Vec<Overlay>, Report) {
    import(xml.as_bytes(), &ImportOptions { name: Some("Demo.scene".into()), base_dir: Some("/work".into()), ..Default::default() }).unwrap()
}
fn seq(p: &Imported) -> &Sequence {
    p.project.sequence(p.sequences[0]).unwrap()
}
fn clips<'a>(p: &'a Imported, name: &str) -> Vec<&'a TrackItem> {
    let mut clips: Vec<_> = seq(p).video_tracks.iter().flat_map(|t| &t.items).filter(|c| c.name == name).collect();
    clips.sort_by_key(|c| c.start);
    clips
}
fn sec(n: i64) -> Tick {
    Tick(n * TICKS_PER_SECOND)
}

#[test]
fn scene_import_video_settings_assets_and_trailing_blank() {
    let (p, o, _) = run(&scene(r#"<layer id="a" asset="v" start="1" end="3"/>"#));
    assert!(o.is_empty());
    assert_eq!(seq(&p).settings.width, 640);
    assert_eq!(seq(&p).duration(), sec(12));
    let c = clips(&p, "a")[0];
    assert_eq!((c.start, c.duration, c.source_in), (sec(1), sec(2), Tick::ZERO));
    let ItemKind::Media(m) = &p.project.item(c.item).unwrap().kind else { panic!() };
    assert_eq!(m.media, MediaRef::File { path: "/work/v.mp4".into() });
}
#[test]
fn scene_import_trim_speed_loop_and_hold() {
    let (p, _, _) = run(&scene(
        r#"<layer id="fast" asset="v" clipIn="2" clipOut="6" speed="2"/><layer id="loop" asset="v" clipIn="2" clipOut="4" loop="2"/><layer id="hold" asset="v" end="4" clipIn="2" speed="0"/>"#,
    ));
    // Four source seconds / 2 = two timeline seconds. loop=2 adds two plays.
    let f = clips(&p, "fast")[0];
    assert_eq!((f.duration, f.source_in, f.speed), (sec(2), sec(2), 2.));
    assert_eq!(
        clips(&p, "loop").iter().map(|c| (c.start, c.duration, c.source_in)).collect::<Vec<_>>(),
        vec![(sec(0), sec(2), sec(2)), (sec(2), sec(2), sec(2)), (sec(4), sec(2), sec(2))]
    );
    assert_eq!(clips(&p, "hold")[0].frame_hold, Some(sec(2)));
}
#[test]
fn scene_import_snap_audio_and_zero_frame() {
    let (p, _, r) = run(&scene(r#"<layer id="a" asset="v" start="0.02" end="1.02"/><layer id="tiny" asset="i" start="2.001" end="2.002"/>"#));
    // At 25 fps .02 and 1.02 are half frames: even rounding gives frame 0 and 26.
    let v = clips(&p, "a")[0];
    assert_eq!((v.start, v.duration), (Tick::ZERO, Tick(TICKS_PER_SECOND * 104 / 100)));
    let a = seq(&p).audio_tracks.iter().flat_map(|t| &t.items).find(|c| c.name == "a").unwrap();
    assert_eq!((a.start, a.duration), (Tick(TICKS_PER_SECOND / 50), sec(1)));
    assert!(v.link.is_some());
    assert_eq!(v.link, a.link);
    assert!(clips(&p, "tiny").is_empty());
    assert!(r.mentions("zero frames"));
    assert_eq!(r.entries.iter().find(|e| e.message == "Video edge snapped to frame grid").unwrap().count, 4);
    assert!(r.mentions("5080320000 ticks"));
}
#[test]
fn scene_import_rational_fps() {
    let (p, _, _) = run(&scene(r#"<layer id="a" asset="v" end="1"/>"#).replace("fps=\"25\"", "fps=\"30000/1001\""));
    // 1 second is nearest frame 30: 30*1001/30000 = 1.001 seconds.
    assert_eq!(seq(&p).settings.frame_rate.num, 30000);
    assert_eq!(clips(&p, "a")[0].duration, Tick(TICKS_PER_SECOND * 1001 / 1000));
}
#[test]
fn scene_import_background_and_stacking() {
    let (p, _, _) = run(&scene(r#"<layer id="a" asset="i" end="5"/><layer id="b" asset="i" end="10"/><layer id="c" asset="i" start="5" end="10"/>"#));
    // The full-duration matte is below A/B/C; C overlaps B despite touching A.
    assert_eq!(seq(&p).video_tracks.len(), 4);
    for (i, name) in [(1, "a"), (2, "b"), (3, "c")] {
        assert_eq!(seq(&p).video_tracks[i].items[0].name, name);
    }
    let (p, _, _) = run(&scene("").replace("duration=\"12\"", "duration=\"12\" background=\"#00000000\""));
    assert_eq!(seq(&p).duration(), sec(12));
    assert!(seq(&p).video_tracks.iter().flat_map(|t| &t.items).all(|c| !c.enabled));
}
#[test]
fn scene_import_anchor_and_inherited_opacity() {
    let (p, _, _) = run(&scene(
        r#"<group id="g" opacity="0.5"><layer id="a" asset="i" x="100" y="200" anchorX="50" anchorY="40" scaleX="2" scaleY="3" rotation="30" opacity="0.5"/></group>"#,
    ));
    let c = clips(&p, "a")[0];
    assert_eq!(c.effect("motion").unwrap().param("anchor").unwrap().value.as_vec2().unwrap(), filmcraft_geom::Vec2::new(50., 40.));
    assert_eq!(c.effect("motion").unwrap().param("position").unwrap().value.as_vec2().unwrap(), filmcraft_geom::Vec2::new(100., 200.));
    assert_eq!(c.effect("opacity").unwrap().param("opacity").unwrap().value.as_f64(), Some(25.));
}
#[test]
fn scene_import_key_bases_and_loop_keys() {
    for (base, times) in [("composition", ("2", "4")), ("local", ("0", "2")), ("normalized", ("0", "1"))] {
        let (p, _, _) = run(&scene(&format!(
            r#"<layer id="a" asset="v" start="2" end="4"><animate property="opacity" timeBase="{base}" defaultInterpolation="hold"><key time="{}" value="0"/><key time="{}" value="1"/></animate></layer>"#,
            times.0, times.1
        )));
        let k = &clips(&p, "a")[0].effect("opacity").unwrap().param("opacity").unwrap().keyframes;
        assert_eq!(k.iter().map(|k| k.time).collect::<Vec<_>>(), vec![sec(0), sec(2)]);
        assert_eq!(k[0].interp, filmcraft_project::Interpolation::Hold);
    }
    let (p, _, _) = run(&scene(
        r#"<layer id="a" asset="v" clipOut="2" loop="2"><animate property="opacity"><key time="0" value="0"/><key time="6" value="1"/></animate></layer>"#,
    ));
    // Keys stay on the composition clock across each repeat; clip-relative offsets shift.
    for (i, c) in clips(&p, "a").iter().enumerate() {
        let k = &c.effect("opacity").unwrap().param("opacity").unwrap().keyframes;
        assert_eq!(k[0].time, sec(-2 * (i as i64)));
        assert_eq!(k[1].time, sec(6 - 2 * (i as i64)));
    }
}
#[test]
fn scene_import_diagnostics_strict_lenient() {
    let xml = scene(r#"<layer id="a" asset="i" bogus="1" another="2"/>"#);
    let d = read::read(&xml, &Default::default()).unwrap();
    let expected = validate::validate(&d.doc);
    let Err(Error::Invalid(diags)) = import(xml.as_bytes(), &Default::default()) else { panic!("must reject with diagnostics") };
    assert_eq!(diags, expected);
    assert!(diags.iter().filter(|d| d.is_error()).count() >= 2);
    let (_, _, r) = import(xml.as_bytes(), &ImportOptions { lenient: true, ..Default::default() }).unwrap();
    assert!(r.mentions("bogus"));
    assert!(r.mentions("another"));
}
#[test]
fn scene_import_overlay_wrappers_assets_order_and_validation() {
    let xml = scene(
        r#"<layer id="before" asset="i"/><group id="clock" start="2" end="10" timeScale="2"><group id="fx" threeD="true" start="4" end="8"><layer id="inside" asset="i"/></group></group><layer id="after" asset="i"/>"#,
    );
    let (p, o, _) = run(&xml);
    assert_eq!(o.len(), 1);
    let nest = clips(&p, "clock")[0];
    let nested = p.project.sequence(nest.item).unwrap();
    let c = nested.video_tracks.iter().flat_map(|t| &t.items).find(|c| c.item == o[0].item).unwrap();
    // Global [3,5) becomes nest [2,6); overlay source remains global at half speed.
    assert_eq!((c.start, c.duration, c.source_in, c.speed), (sec(2), sec(4), sec(3), 0.5));
    assert!(seq(&p).video_tracks[2].items.iter().any(|c| c.item == nest.item));
    assert_eq!(seq(&p).video_tracks[3].items[0].name, "after");
    let s = std::str::from_utf8(&o[0].document).unwrap();
    let d = read::read(s, &Default::default()).unwrap();
    assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
    assert!(s.contains("#00000000"));
    assert!(!s.contains("v.mp4"));
    assert!(s.contains("i.png"));
    assert!(s.contains("timeScale=\"2\""));
    assert!(o[0].path.starts_with("/work/Demo Scene Overlays/"));
    let Err(Error::Other(e)) = crate::import_with(xml.as_bytes(), Format::Scene, &Default::default()) else { panic!() };
    assert!(e.contains("1 scene overlay"));
}

#[test]
fn scene_import_gradient_first_stop_and_audio_volume() {
    let xml=scene(r#"<layer id="a" asset="v" volume="0.5" mute="true"/>"#)
        .replace("duration=\"12\"","duration=\"12\" background=\"url(#sky)\"")
        .replace("<composition>",r##"<paints><linearGradient id="sky"><stop offset="0" color="#FF0000" opacity="0.5"/><stop offset="1" color="#0000FF"/></linearGradient></paints><composition>"##);
    let (p, _, r) = run(&xml);
    let c = &seq(&p).video_tracks[0].items[0];
    let ItemKind::Media(m) = &p.project.item(c.item).unwrap().kind else { panic!() };
    assert_eq!(m.media, MediaRef::Generator(filmcraft_media::Generator::ColorMatte { color: [1., 0., 0., 0.5] }));
    assert!(r.mentions("gradient background"));
    let c = &seq(&p).audio_tracks[0].items[0];
    assert_eq!(c.gain_db, -287.5);
    // amplitude 1/2 is 20 log10(1/2) dB.
    assert!((c.effect("volume").unwrap().param("level").unwrap().value.as_f64().unwrap() + 6.020599913279624).abs() < 1e-10);
}
#[test]
fn scene_import_remap_freeze_and_reverse() {
    let (p, _, r) = run(&scene(
        r#"<layer id="remap" asset="v" end="4" freezeAt="1"><timeRemap><key time="0" value="2"/><key time="2" value="4"/></timeRemap></layer><layer id="reverse" asset="v" clipIn="2" clipOut="6" reverse="true"/>"#,
    ));
    // freezeAt=1 selects source 3 on the resolved local remap line (2 -> 4 over 2s).
    assert_eq!(clips(&p, "remap")[0].frame_hold, Some(sec(3)));
    assert!(r.mentions("timeRemap"));
    let c = clips(&p, "reverse")[0];
    assert!(c.reverse);
    assert_eq!(c.source_time_at(c.start), sec(6));
}
#[test]
fn scene_import_overlay_sequence_placement_and_transitive_assets() {
    let xml =
        scene(r#"<sequence id="s"><layer id="a" asset="i" end="3"/><group id="fx" threeD="true" end="2"><layer id="inside" asset="i"/></group></sequence>"#);
    let (p, o, _) = run(&xml);
    let c = seq(&p).video_tracks.iter().flat_map(|t| &t.items).find(|c| c.item == o[0].item).unwrap();
    // The sequence is retained intact; its carrier covers the enclosing interval.
    assert_eq!((c.start, c.duration), (sec(0), sec(12)));
    assert!(clips(&p, "a").is_empty());
    let doc = read::read(std::str::from_utf8(&o[0].document).unwrap(), &Default::default()).unwrap();
    assert!(validate::validate(&doc.doc).iter().all(|d| !d.is_error()));
    let r = resolve::resolve(&doc, &Default::default()).unwrap();
    let fx = r.nodes.iter().find(|n| n.id == "fx").unwrap();
    assert_eq!((fx.interval.start, fx.interval.end), (3., 5.));
}
#[test]
fn scene_import_lenient_dispatch_drops_overlay_clips() {
    let xml = scene(r#"<group id="fx" threeD="true"><layer id="inside" asset="v"/></group>"#);
    let (p, r) = crate::import_with(xml.as_bytes(), Format::Scene, &ImportOptions { lenient: true, ..Default::default() }).unwrap();
    assert!(r.mentions("dropped"));
    assert!(!p.project.items.values().any(|i| matches!(&i.kind,ItemKind::Media(m) if matches!(&m.media,MediaRef::File{path} if path.ends_with(".scene")))));
}
#[test]
fn scene_import_unmapped_counts_and_expression_overlay() {
    let (p, o, r) = run(&scene(
        r#"<layer id="a" asset="i"><expression property="opacity">0.5</expression></layer><layer id="b" asset="i"><expression property="opacity">0.2</expression></layer>"#,
    ));
    assert_eq!(o.len(), 2);
    assert_eq!(r.entries.iter().find(|e| e.message == "Unmapped <expression>").unwrap().count, 2);
    for overlay in o {
        let doc = read::read(std::str::from_utf8(&overlay.document).unwrap(), &Default::default()).unwrap();
        assert!(validate::validate(&doc.doc).iter().all(|d| !d.is_error()));
        assert!(seq(&p).video_tracks.iter().flat_map(|t| &t.items).any(|c| c.item == overlay.item));
    }
}
#[test]
fn scene_import_sanitizes_overlay_names() {
    assert_eq!(super::overlay::sanitize("../x"), "___x");
    assert_eq!(super::overlay::sanitize(&"é".repeat(100)).len(), 64);
    let (_, o, _) = import(
        scene(r#"<group id="fx" threeD="true"/>"#).as_bytes(),
        &ImportOptions { name: Some("../../evil.scene".into()), base_dir: Some("/safe".into()), ..Default::default() },
    )
    .unwrap();
    assert!(o[0].path.starts_with("/safe/evil Scene Overlays/"));
}
#[test]
fn scene_import_hostile_limits_and_nonfinite_even_lenient() {
    let opts = ImportOptions { lenient: true, ..Default::default() };
    for xml in [
        scene(r#"<layer id="a" asset="v" clipOut="1" loop="999999999999999"/>"#),
        scene(r#"<layer id="a" asset="v" speed="NaN"/>"#),
        scene(r#"<layer id="a" asset="i" parent="b"/><layer id="b" asset="i" parent="a"/>"#),
        scene(&format!("{}{}", "<group>".repeat(300), "</group>".repeat(300))),
        "<scene".into(),
    ] {
        let result = std::panic::catch_unwind(|| import(xml.as_bytes(), &opts));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }
}
proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(32))]
    #[test]
    fn scene_import_random_small_documents_never_panic(start in -10i32..20, end in -10i32..20, speed in -4i32..5, count in 0u32..5, corrupt in proptest::bool::ANY) {
        let mut xml=scene(&format!(r#"<layer id="a" asset="v" start="{start}" end="{end}" speed="{speed}" loop="{count}"/>"#));
        if corrupt {xml.truncate(xml.len()/2);}
        let result=std::panic::catch_unwind(||import(xml.as_bytes(),&ImportOptions{lenient:true,..Default::default()}));
        proptest::prop_assert!(result.is_ok());
    }
}

#[test]
fn scene_import_image_sequence_declared_metadata() {
    let xml = r#"<scene version="1.7"><project width="640" height="480" fps="25" duration="4"/><assets><imageSequence id="seq" src="img_%04d.png" first="10" last="18" step="2" fps="25" width="100" height="80"/></assets><composition><layer id="a" asset="seq"/></composition></scene>"#;
    let (p, _, _) = run(xml);
    let c = clips(&p, "a")[0];
    let ItemKind::Media(m) = &p.project.item(c.item).unwrap().kind else { panic!() };
    // Frames 10,12,14,16,18 = 5 frames / 25 fps = .2s.
    assert_eq!(m.info.duration, Tick(TICKS_PER_SECOND / 5));
    assert_eq!(c.duration, m.info.duration);
}
#[test]
fn scene_import_overlay_transitive_symbol_references() {
    let xml = scene(r#"<instance id="fx" symbol="symbol" start="2" end="4"/>"#)
        .replace("<composition>", r#"<symbols><symbol id="symbol" duration="2"><layer id="inside" asset="i"/></symbol></symbols><composition>"#);
    let (_, o, _) = run(&xml);
    assert_eq!(o.len(), 1);
    let text = std::str::from_utf8(&o[0].document).unwrap();
    let d = read::read(text, &Default::default()).unwrap();
    assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
    assert!(text.contains("i.png"));
    assert!(!text.contains("v.mp4"));
    assert!(text.contains("<symbol "));
}
#[test]
fn scene_import_unmapped_attributes_are_counted() {
    let (_, _, r) = run(&scene(r#"<layer id="a" asset="i" blend="multiply"/><layer id="b" asset="i" blend="multiply"/>"#));
    assert_eq!(r.entries.iter().find(|e| e.message == "Unmapped layer@blend").unwrap().count, 2);
}
#[test]
fn scene_import_lenient_nonfinite_unmapped_attribute_is_error() {
    let xml = scene(r#"<layer id="a" asset="i" z="NaN"/>"#);
    let result = std::panic::catch_unwind(|| import(xml.as_bytes(), &ImportOptions { lenient: true, ..Default::default() }));
    assert!(result.is_ok());
    assert!(result.unwrap().is_err());
}
#[test]
fn scene_import_css_bezier_and_local_keys_under_group_clock() {
    let (p, _, r) = run(&scene(
        r#"<group id="g" start="2" end="10" timeOffset="1" timeScale="2"><layer id="a" asset="i" start="4" end="8"><animate property="opacity" timeBase="local"><key time="0" value="0" interpolation="cubic-bezier" bezier="0.42,0,1,1" easeOut="0.9,0.9" spatialOut="3,4"/><key time="2" value="1"/></animate></layer></group>"#,
    ));
    // Composition t=2+u/2. Local keys u=4 and 6 => t=4 and 5; media rate 2 => keys 0 and 2.
    let group = clips(&p, "g")[0];
    let nested = p.project.sequence(group.item).unwrap();
    let child = nested.video_tracks.iter().flat_map(|t| &t.items).find(|c| c.name == "a").unwrap();
    let opacity = child.effect("opacity").unwrap().param("opacity").unwrap();
    let keys = &opacity.keyframes;
    // One composition second at 25 fps gives 25 intervals, including both endpoint keys.
    assert_eq!(keys.len(), 26);
    assert_eq!(keys.first().unwrap().time, sec(0));
    assert_eq!(keys.last().unwrap().time, sec(2));
    assert!(keys.iter().all(|k| k.interp == filmcraft_project::Interpolation::Linear));
    for frame in 1..25 {
        // At composition 4 + frame/25, the group clock is 4 + 2*frame/25:
        // segment progress is frame/25 and the media clock is 2*frame/25 seconds.
        let progress = f64::from(frame) / 25.;
        // Independently invert x(s)=1.26s+0.48s²-0.74s³ with Newton iteration;
        // CSS (0.42,0,1,1) gives y(s)=3s²-2s³. No production solver is used.
        let mut s = progress;
        for _ in 0..16 {
            let x = s * (1.26 + s * (0.48 - 0.74 * s));
            let derivative = 1.26 + s * (0.96 - 2.22 * s);
            s -= (x - progress) / derivative;
        }
        assert!((s * (1.26 + s * (0.48 - 0.74 * s)) - progress).abs() < 1e-12);
        let expected = 100. * s * s * (3. - 2. * s);
        let media_time = Tick(2 * TICKS_PER_SECOND * i64::from(frame) / 25);
        let actual = opacity.f64_at(media_time);
        assert!((actual - expected).abs() < 1e-6, "frame {frame}: {actual} != {expected}");
    }
    assert!(r.mentions("CSS bezier"));
    assert!(r.mentions("baked at sequence frame rate"));
    assert!(r.mentions("spatialOut"));
}
#[test]
fn scene_import_overlays_bound_aggregate_work() {
    // Every overlay requires another document validation. Bound the product of document nodes
    // and overlay roots, including empty roots whose output byte size alone would not bound it.
    let nodes = (0..1500).map(|i| format!(r#"<group id="g{i}" threeD="true"/>"#)).collect::<String>();
    assert!(import(scene(&nodes).as_bytes(), &Default::default()).is_err());
}

#[test]
fn scene_import_background_edges_snap() {
    // 1.02 s * 25 = 25.5 frames, ties-to-even chooses 26 frames = 1.04 s.
    let (p, _, r) = run(&scene("").replace("duration=\"12\"", "duration=\"1.02\""));
    assert_eq!(seq(&p).video_tracks[0].items[0].duration, Tick(TICKS_PER_SECOND * 104 / 100));
    assert_eq!(seq(&p).mark_out, Some(Tick(TICKS_PER_SECOND * 102 / 100)));
    assert!(r.mentions("snapped"));
}
#[test]
fn scene_import_overlay_ancestor_reference_does_not_copy_siblings() {
    let (_, o, _) = run(&scene(
        r#"<group id="g"><layer id="sibling" asset="v"/><layer id="fx" asset="i" parent="g"><expression property="opacity">0.5</expression></layer></group>"#,
    ));
    assert_eq!(o.len(), 1);
    let text = std::str::from_utf8(&o[0].document).unwrap();
    assert!(!text.contains("sibling"));
    assert!(!text.contains("v.mp4"));
    let d = read::read(text, &Default::default()).unwrap();
    assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
}
#[test]
fn scene_import_overlay_external_expression_dependency_preserved_once() {
    // The dependency promotes fx to the whole composition [0,12). The earlier
    // overlay and native sibling must each occur once, inside that one overlay.
    let xml = scene(
        r#"<layer id="first" asset="i"><expression property="opacity">0.5</expression></layer><layer id="other" asset="i"/><layer id="fx" asset="i" start="3" end="5" visible="false"><expression property="opacity">prop('other.opacity')</expression></layer>"#,
    );
    let xml = xml.replace("duration=\"12\"", "duration=\"12\" background=\"#00000000\"");
    let (p, overlays, _) = run(&xml);
    assert_eq!(overlays.len(), 1, "promoted overlays must coalesce");
    let represented: Vec<_> = seq(&p).video_tracks.iter().flat_map(|t| &t.items).filter(|c| c.name != "Scene Background").collect();
    assert_eq!(represented.len(), 1, "covered native siblings must not render twice");
    let clip = represented[0];
    assert_eq!(clip.item, overlays[0].item);
    assert_eq!((clip.start, clip.duration, clip.source_in, clip.enabled), (sec(0), sec(12), sec(0), true));
    let doc = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
    for id in ["first", "other", "fx"] {
        assert_eq!(doc.doc.descendants().filter(|n| n.attribute("id") == Some(id)).count(), 1);
    }
    assert_overlay_round_trip(&p);
}

fn assert_overlay_round_trip(p: &Imported) {
    let (bytes, _) = crate::export(&p.project, p.sequences[0], Format::Scene, &Default::default()).unwrap();
    let (again, _, _) = import(&bytes, &Default::default()).unwrap();
    // These fixtures have a transparent background. Re-import adds the XSD's
    // default matte; exclude it just as the corpus harness does.
    let before = seq(p).all_tracks().flat_map(|t| &t.items).count();
    let after = seq(&again).all_tracks().flat_map(|t| &t.items).filter(|c| c.name != "Scene Background").count();
    assert_eq!(after, before, "export/re-import must keep the clip count");
}

#[test]
fn scene_import_overlay_promoted_group_interval_and_clocks() {
    // Group [2,10), child clock = 2 + (t - 2 - 2)*2 = 2t-6.
    // Source [4,8) is global [5,7); fx [6,8) is [6,7). Promotion
    // must keep the whole group [2,10), including its retimed children.
    let xml = scene(
        r#"<layer id="before" asset="i"/><group id="clock" start="2" end="10" timeScale="2" timeOffset="2"><layer id="other" asset="i" start="4" end="8"/><group id="nested" isolate="true"><layer id="native" asset="i"/></group><layer id="fx" asset="i" start="6" end="8"><expression property="opacity">prop('other.opacity')</expression></layer></group><layer id="after" asset="i"/>"#,
    );
    let xml = xml.replace("duration=\"12\"", "duration=\"12\" background=\"#00000000\"");
    let (p, overlays, _) = run(&xml);
    assert_eq!(overlays.len(), 1);
    let clip = seq(&p).video_tracks.iter().flat_map(|t| &t.items).find(|c| c.item == overlays[0].item).expect("enclosing group must be the overlay carrier");
    assert_eq!((clip.name.as_str(), clip.start, clip.duration, clip.source_in, clip.speed), ("clock", sec(2), sec(8), sec(2), 1.));
    assert_eq!(p.project.items.values().filter(|i| i.as_sequence().is_some()).count(), 1, "covered nested groups must not create empty carriers");
    assert_eq!(seq(&p).video_tracks.iter().flat_map(|t| &t.items).map(|c| c.name.as_str()).collect::<Vec<_>>(), ["before", "clock", "after"]);
    let doc = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
    let r = resolve::resolve(&doc, &Default::default()).unwrap();
    let source = r.nodes.iter().find(|n| n.id == "other").unwrap();
    assert_eq!((source.interval.start, source.interval.end), (5., 7.));
    assert_overlay_round_trip(&p);
}

#[test]
fn scene_import_overlay_promoted_sequence_keeps_placement() {
    // sequence children auto-place: first [0,3), second [3,5).
    let xml = scene(
        r#"<sequence id="s"><layer id="first" asset="i" end="3"/><layer id="second" asset="i" end="2"><expression property="opacity">prop('first.opacity')</expression></layer></sequence>"#,
    );
    let xml = xml.replace("duration=\"12\"", "duration=\"12\" background=\"#00000000\"");
    let (p, overlays, _) = run(&xml);
    assert_eq!(overlays.len(), 1);
    assert_eq!(clips(&p, "first").len(), 0, "auto-placed sibling is already in the overlay");
    let clip = seq(&p).video_tracks.iter().flat_map(|t| &t.items).find(|c| c.item == overlays[0].item).unwrap();
    assert_eq!((clip.name.as_str(), clip.start, clip.duration), ("s", sec(0), sec(12)));
    let doc = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
    let r = resolve::resolve(&doc, &Default::default()).unwrap();
    let second = r.nodes.iter().find(|n| n.id == "second").unwrap();
    assert_eq!((second.interval.start, second.interval.end), (3., 5.));
    assert_overlay_round_trip(&p);
}

#[test]
fn scene_import_overlay_dynamic_dependency_refused() {
    // A computed target cannot be dependency-closed without evaluating expressions.
    let xml = scene(r#"<layer id="fx" asset="i"><expression property="opacity">prop(target)</expression></layer>"#);
    assert!(import(xml.as_bytes(), &Default::default()).unwrap_err().to_string().contains("dynamic expression dependency"));
}

#[test]
fn scene_import_overlay_expression_parameter_closure() {
    let xml = scene(r#"<layer id="fx" asset="i"><expression property="opacity">param('amount')</expression></layer>"#).replace(
        "<assets>",
        r#"<parameters><param id="amount" type="number" default="0.5"/><param id="unused" type="number" default="1"/></parameters><assets>"#,
    );
    let (_, o, _) = run(&xml);
    let text = std::str::from_utf8(&o[0].document).unwrap();
    assert!(text.contains("id=\"amount\""));
    assert!(!text.contains("unused"));
    let d = read::read(text, &Default::default()).unwrap();
    assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
}

#[test]
fn scene_import_animated_child_parent_limitation_reported() {
    let (_, _, r) = run(&scene(
        r#"<group id="g" x="10"><layer id="a" asset="i"><animate property="x"><key time="0" value="0"/><key time="1" value="10"/></animate></layer></group>"#,
    ));
    // S4 intentionally omits world_matrix for animated transforms, even with a static parent.
    assert!(r.mentions("parent transform on animated Motion"));
}
#[test]
fn scene_import_declared_video_alpha() {
    let (p, _, _) = run(&scene(r#"<layer id="a" asset="v"/>"#).replace("hasAudio=\"true\"", "hasAudio=\"true\" alpha=\"straight\""));
    let c = clips(&p, "a")[0];
    let ItemKind::Media(m) = &p.project.item(c.item).unwrap().kind else { panic!() };
    assert!(m.info.video.as_ref().unwrap().has_alpha);
}

#[test]
fn scene_import_overlay_transitive_audio_is_reported() {
    let xml = scene(r#"<instance id="fx" symbol="symbol" start="2" end="4"/>"#)
        .replace("<composition>", r#"<symbols><symbol id="symbol" duration="2"><layer id="inside" asset="v"/></symbol></symbols><composition>"#);
    let (_, o, r) = run(&xml);
    assert_eq!(o.len(), 1);
    assert!(r.mentions("overlay contains audio"));
}

#[test]
fn scene_import_overlay_relative_base_paths() {
    let xml = scene(r#"<group id="fx" threeD="true"><layer id="inside" asset="i"/></group>"#);
    let (_, o, _) = import(xml.as_bytes(), &ImportOptions { base_dir: Some("project".into()), name: Some("Demo.scene".into()), ..Default::default() }).unwrap();
    let text = std::str::from_utf8(&o[0].document).unwrap();
    let d = read::read(text, &Default::default()).unwrap();
    assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
    // The overlay is project/Demo Scene Overlays/*.scene: its image is one level up.
    assert_eq!(d.doc.descendants().find(|n| n.has_tag_name("image")).unwrap().attribute("src"), Some("../i.png"));
}

#[test]
fn scene_import_review_1_dispatch_before_text_allocation() {
    // Safe allocation instrumentation would require a new dependency or an unsafe global
    // allocator. Guard the dispatch order as well as the observable size-limit error.
    let source = include_str!("../../lib.rs");
    let body = source.split("pub fn import_with(").nth(1).unwrap().split("fn decode_text").next().unwrap();
    assert!(body.find("scene::import(bytes, opts)").unwrap() < body.find("decode_text(bytes)").unwrap(), "scene limit must precede text allocation");
    let bytes = vec![0xff; read::Limits::default().max_bytes + 1];
    for lenient in [false, true] {
        assert!(crate::import_with(&bytes, Format::Scene, &ImportOptions { lenient, ..Default::default() }).is_err());
    }
}

fn opacity_at(c: &TrackItem, t: Tick) -> f64 {
    c.effect("opacity").unwrap().param("opacity").unwrap().f64_at(c.effect_time_at(t))
}

#[test]
fn scene_import_review_2_media_clock_trim_speed_reverse_and_loops() {
    for (extra, clip_out) in [("", 4), ("speed=\"2\"", 6), ("reverse=\"true\"", 4), ("speed=\"2\" reverse=\"true\"", 6), ("loop=\"2\"", 4)] {
        let (p, _, _) = run(&scene(&format!(
            r#"<layer id="a" asset="v" clipIn="2" clipOut="{clip_out}" {extra}><animate property="opacity" timeBase="local"><key time="0" value="0"/><key time="2" value="1"/></animate></layer>"#
        )));
        // Local fade runs from composition 0 to 2, independent of source trim/rate.
        for c in clips(&p, "a") {
            for t in [c.start, c.start + sec(1), c.end()] {
                let expected = ((t.0 as f64 / sec(2).0 as f64) * 100.).min(100.);
                assert!((opacity_at(c, t) - expected).abs() < 1e-8, "{extra}: at {t:?}, opacity {} != {expected}", opacity_at(c, t));
            }
        }
    }
}

#[test]
fn scene_import_review_2_hold_animates_effects() {
    for mode in ["speed=\"0\"", "freezeAt=\"1\""] {
        let (p, _, _) = run(&scene(&format!(
            r#"<layer id="a" asset="v" clipIn="2" end="2" {mode}><animate property="opacity" timeBase="local"><key time="0" value="0"/><key time="2" value="1"/></animate></layer>"#
        )));
        let c = clips(&p, "a")[0];
        // XSD layerType freezes the source frame, not the layer's animated properties.
        assert!(!c.hold_filters);
        assert_eq!(c.source_time_at(sec(0)), c.source_time_at(sec(2)));
        assert_eq!(opacity_at(c, sec(0)), 0.);
        assert_eq!(opacity_at(c, sec(1)), 50.);
        assert_eq!(opacity_at(c, sec(2)), 100.);
    }
}

#[test]
fn scene_import_review_3_snapped_video_keeps_audio_source_phase() {
    for reverse in [false, true] {
        let (p, _, _) = run(&scene(&format!(r#"<layer id="a" asset="v" start="0.02" end="1.02" clipIn="1" clipOut="5" speed="2" reverse="{reverse}"/>"#)));
        let v = clips(&p, "a")[0];
        let a = seq(&p).audio_tracks.iter().flat_map(|t| &t.items).find(|c| c.name == "a").unwrap();
        // At .04s: forward source=1+(.04-.02)*2=1.04; reverse=5-.04=4.96.
        let t = Tick(TICKS_PER_SECOND / 25);
        let expected = Tick(TICKS_PER_SECOND * if reverse { 496 } else { 104 } / 100);
        assert!((v.source_time_at(t).0 - expected.0).abs() <= 1);
        assert!((v.source_time_at(t).0 - a.source_time_at(t).0).abs() <= 1);
    }
}

#[test]
fn scene_import_review_4_complete_symbol_sequence_stays_intact() {
    let xml = scene(r#"<instance id="fx" symbol="symbol" start="2" end="8"/>"#).replace("<composition>", r#"<symbols><symbol id="symbol" duration="6"><sequence id="seq" timeGap="1" transition="crossfade" transitionDuration="0.5"><layer id="first" asset="i" end="2"/><layer id="second" asset="i" end="3"/></sequence></symbol></symbols><composition>"#);
    let (_, overlays, _) = run(&xml);
    let text = std::str::from_utf8(&overlays[0].document).unwrap();
    let d = read::read(text, &Default::default()).unwrap();
    assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
    let sequence = d.doc.descendants().find(|n| n.attribute("id") == Some("seq")).unwrap();
    // Preserve sequence semantics: first [0,2), second [3,6), with its default transition.
    assert!(sequence.has_tag_name("sequence"));
    assert_eq!(sequence.attribute("timeGap"), Some("1"));
    assert_eq!(sequence.attribute("transition"), Some("crossfade"));
    assert_eq!(sequence.attribute("transitionDuration"), Some("0.5"));
    assert_eq!(sequence.children().filter(Node::is_element).count(), 2);
}

#[test]
fn scene_import_review_5_unknown_media_duration_from_source_uses() {
    for (asset, lenient) in [(r#"<video id="v" src="v.mp4" width="640" height="480" fps="25"/>"#, true), (r#"<audio id="v" src="a.wav"/>"#, false)] {
        for (mode, expected) in [("speed=\"2\"", sec(7)), ("speed=\"0\"", sec(5)), ("speed=\"2\" reverse=\"true\" clipOut=\"7\"", Tick(sec(7).0 + 1))] {
            let xml = format!(
                r#"<scene version="1.7"><project width="640" height="480" fps="25" duration="2"/><assets>{asset}</assets><composition><layer id="a" asset="v" clipIn="3" end="2" {mode}/></composition></scene>"#
            );
            let (p, _, _) = import(xml.as_bytes(), &ImportOptions { lenient, ..Default::default() }).unwrap();
            let m = p.project.items.values().find(|i| i.name == "v").unwrap().as_media().unwrap();
            // Two seconds at speed 2 consume four source seconds. A hold keeps its source frame
            // at 3 but its moving effect clock reaches 5; both bounds must fit the media.
            assert_eq!(m.info.duration, expected, "{asset} {mode}");
        }
    }
}

#[test]
fn scene_import_review_6_overlay_font_family_closure() {
    for text_attrs in [r#"font="Test" fallback="Fallback""#, r#"style="derived""#] {
        let xml = scene(r#"<group id="fx" threeD="true"><layer id="label" asset="text"/></group>"#)
            .replace("duration=\"12\"", "duration=\"12\" fontPolicy=\"pinned\"")
            .replace("<assets>", r#"<styles><textStyle id="base" font="Test" fallback="Fallback"/><textStyle id="derived" basedOn="base"/></styles><assets>"#)
            .replace("</assets>", &format!(r#"<font sha256="0000000000000000000000000000000000000000000000000000000000000000" id="font1" src="test.ttf" family="Test"/><font sha256="0000000000000000000000000000000000000000000000000000000000000000" id="font2" src="fallback.ttf" family="Fallback"/><font sha256="0000000000000000000000000000000000000000000000000000000000000000" id="unused" src="unused.ttf" family="Unused"/><text id="text" width="100" height="40" size="20" {text_attrs} text="Hello"/></assets>"#));
        let (_, overlays, _) = run(&xml);
        let text = std::str::from_utf8(&overlays[0].document).unwrap();
        let d = read::read(text, &Default::default()).unwrap();
        assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
        assert!(text.contains("test.ttf"));
        assert!(text.contains("fallback.ttf"));
        assert!(!text.contains("unused.ttf"));
    }
}

#[test]
fn scene_import_review_7_overlay_font_file_relocated() {
    let xml = scene(r#"<group id="fx" threeD="true"><layer id="label" asset="text"/></group>"#)
        .replace("</assets>", r#"<text id="text" width="100" height="40" size="20" fontFile="fonts/test.ttf" text="Hello"/></assets>"#);
    for (base, expected) in [("project", "../fonts/test.ttf"), ("/work", "/work/fonts/test.ttf"), (r"C:\work", r"C:\work\fonts\test.ttf")] {
        let (_, overlays, _) = import(xml.as_bytes(), &ImportOptions { base_dir: Some(base.into()), ..Default::default() }).unwrap();
        let d = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
        assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
        assert_eq!(d.doc.descendants().find(|n| n.has_tag_name("text")).unwrap().attribute("fontFile"), Some(expected));
    }
}

fn audio_clip(p: &Imported) -> &TrackItem {
    seq(p).audio_tracks.iter().flat_map(|t| &t.items).find(|c| c.name == "a").unwrap()
}
fn amplitude_at(c: &TrackItem, t: Tick) -> f64 {
    let db = c.effect("volume").unwrap().param("level").unwrap().f64_at(c.effect_time_at(t));
    10f64.powf((db + c.gain_db) / 20.)
}
#[test]
fn scene_import_review_8_zero_volume_can_animate_to_audible() {
    let (p, _, _) = run(&scene(
        r#"<layer id="a" asset="v" end="2" volume="0"><animate property="volume" timeBase="local"><key time="0" value="0"/><key time="2" value="1"/></animate></layer>"#,
    ));
    let c = audio_clip(&p);
    assert!((amplitude_at(c, sec(2)) - 1.).abs() < 1e-8);
}
#[test]
fn scene_import_review_9_volume_fade_is_amplitude_linear() {
    let (p, _, report) = run(&scene(
        r#"<layer id="a" asset="v" end="2"><animate property="volume" timeBase="local"><key time="0" value="0"/><key time="2" value="1"/></animate></layer>"#,
    ));
    let c = audio_clip(&p);
    // An amplitude-linear 0->1 fade is .5 at one second, not a linear dB fade.
    assert!((amplitude_at(c, sec(1)) - 0.5).abs() < 0.001);
    for i in 0..=4000 {
        let t = Tick(sec(2).0 * i / 4000);
        assert!((amplitude_at(c, t) - i as f64 / 4000.).abs() <= 0.001);
    }
    assert!(report.mentions("volume") && report.mentions("approximat"));
}

#[test]
fn scene_import_review_7_overlay_keeps_builtin_uri() {
    let xml = scene(r#"<group id="fx" threeD="true"/>"#).replace("<assets>", r#"<colorManagement ocioConfig="ocio://studio-config-v2"/><assets>"#);
    let (_, overlays, _) = run(&xml);
    let d = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
    assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
    assert_eq!(d.doc.descendants().find(|n| n.has_tag_name("colorManagement")).unwrap().attribute("ocioConfig"), Some("ocio://studio-config-v2"));
}

#[test]
fn scene_import_review_4_retained_sequence_keeps_animation() {
    let xml = scene(
        r#"<sequence id="s"><animate property="opacity"><key time="0" value="0"/><key time="4" value="1"/></animate><layer id="a" asset="i" end="2"/><group id="fx" threeD="true" end="2"><layer id="inside" asset="i"/></group></sequence>"#,
    );
    let (_, overlays, _) = run(&xml);
    let d = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
    assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
    let sequence = d.doc.descendants().find(|n| n.attribute("id") == Some("s")).unwrap();
    assert!(sequence.has_tag_name("sequence"));
    assert!(sequence.children().any(|c| c.has_tag_name("animate")));
    let resolved = resolve::resolve(&d, &Default::default()).unwrap();
    let fx = resolved.nodes.iter().find(|n| n.id == "fx").unwrap();
    assert_eq!((fx.interval.start, fx.interval.end), (2., 4.));
}

#[test]
fn scene_import_review_2_reverse_preserves_outgoing_hold() {
    let (p, _, _) = run(&scene(
        r#"<layer id="a" asset="v" clipIn="2" clipOut="6" reverse="true"><animate property="opacity" timeBase="local"><key time="0" value="0.2" interpolation="hold"/><key time="2" value="1"/><key time="4" value="0"/></animate></layer>"#,
    ));
    let c = clips(&p, "a")[0];
    for (time, expected) in [(Tick::ZERO, 20.), (sec(1), 20.), (Tick(sec(2).0 - 1), 20.), (sec(2), 100.), (sec(3), 50.), (sec(4), 0.)] {
        assert!((opacity_at(c, time) - expected).abs() < 1e-8, "{time:?}");
    }
}

#[test]
fn scene_import_review_7_svg_path_is_not_a_filename() {
    let xml = scene(r#"<group id="fx" threeD="true"><shape id="s" shape="path" width="10" height="10" path="M0 0 L10 0 L10 10 Z"/></group>"#);
    let (_, overlays, _) = run(&xml);
    let d = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
    assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
    assert_eq!(d.doc.descendants().find(|n| n.has_tag_name("shape")).unwrap().attribute("path"), Some("M0 0 L10 0 L10 10 Z"));
}

#[test]
fn scene_import_review_5_hold_moving_clock_overflow_is_rejected() {
    let (mut p, _, _) = run(&scene(r#"<layer id="a" asset="v" end="2" speed="0"/>"#));
    let sequence = p.sequences[0];
    let clip = p.project.sequence_mut(sequence).unwrap().video_tracks.iter_mut().flat_map(|t| &mut t.items).find(|c| c.name == "a").unwrap();
    // A held frame at MAX-1 fits, but its moving 2-second effect clock does not.
    clip.source_in = Tick(i64::MAX - 1);
    clip.frame_hold = Some(clip.source_in);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fill_unknown_durations(&mut p.project)));
    assert!(result.is_ok());
    assert!(result.unwrap().is_err());
}

#[test]
fn scene_import_review_7_image_sequence_pattern_relocated() {
    for pattern in ["frames/frame_%04d.png", "####.png"] {
        let xml = scene(r#"<group id="fx" threeD="true"><layer id="inside" asset="frames"/></group>"#)
            .replace("</assets>", &format!(r#"<imageSequence id="frames" src="{pattern}" first="0" last="9" fps="25" width="10" height="10"/></assets>"#));
        let (_, overlays, _) = run(&xml);
        let d = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
        assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
        assert_eq!(d.doc.descendants().find(|n| n.has_tag_name("imageSequence")).unwrap().attribute("src"), Some(format!("/work/{pattern}").as_str()));
    }
}

#[test]
fn scene_import_review_7_basemap_style_file_and_builtin() {
    for (style, expected) in [("styles/map.json", "/work/styles/map.json"), ("protomaps-light", "protomaps-light"), ("protomaps-dark", "protomaps-dark")] {
        let xml = scene(r#"<group id="fx" threeD="true"><layer id="inside" asset="map"/></group>"#).replace(
            "</assets>",
            &format!(
                r#"<tiles id="tiles" src="map.pmtiles"/><map id="map" width="640" height="480"><basemap tiles="tiles" mapStyle="{style}"/></map></assets>"#
            ),
        );
        let (_, overlays, _) = run(&xml);
        let d = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
        assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
        assert_eq!(d.doc.descendants().find(|n| n.has_tag_name("basemap")).unwrap().attribute("mapStyle"), Some(expected));
    }
}

#[test]
fn scene_import_review_7_caption_model_file_and_name() {
    for (model, expected) in [("models/whisper.bin", "/work/models/whisper.bin"), ("whisper.bin", "/work/whisper.bin"), ("base.en", "base.en")] {
        let xml = scene(r#"<layer id="fx" asset="i"><expression property="opacity">prop('ct.opacity')</expression></layer>"#)
            .replace("</scene>", &format!(r#"<captions><captionTrack id="ct" language="en" src="captions.srt" model="{model}"/></captions></scene>"#));
        let (_, overlays, _) = run(&xml);
        let d = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
        assert!(validate::validate(&d.doc).iter().all(|d| !d.is_error()));
        assert_eq!(d.doc.descendants().find(|n| n.has_tag_name("captionTrack")).unwrap().attribute("model"), Some(expected));
    }
}

#[test]
fn scene_import_overlay_review_covered_audio_survives() {
    // Video-only overlays must not eat the original soundtrack, even when
    // the audio-bearing layer is the composition overlay's anchor.
    for nodes in [
        r#"<layer id="other" asset="v" start="3" end="5" clipIn="2"/><layer id="fx" asset="i"><expression property="x">prop('other.x')</expression></layer>"#,
        r#"<group id="clock" start="2" end="10" timeOffset="2" timeScale="2"><layer id="other" asset="v" start="4" end="8" clipIn="2"/><layer id="fx" asset="i"><expression property="x">prop('other.x')</expression></layer></group>"#,
    ] {
        let xml = scene(nodes).replace("duration=\"12\"", "duration=\"10\" background=\"#00000000\"");
        let (p, overlays, _) = run(&xml);
        assert_eq!(overlays.len(), 1);
        let audio: Vec<_> = seq(&p).audio_tracks.iter().flat_map(|t| &t.items).collect();
        assert_eq!(audio.len(), 1, "covered video must retain its audio once");
        let (start, duration, speed) = if nodes.starts_with("<group") { (sec(5), sec(2), 2.) } else { (sec(3), sec(2), 1.) };
        assert_eq!((audio[0].start, audio[0].duration, audio[0].source_in, audio[0].speed), (start, duration, sec(2), speed));
        assert_eq!(audio[0].link, None, "no dangling link to suppressed native video");
        assert!(clips(&p, "other").is_empty());
        assert_overlay_round_trip(&p);
    }
}

#[test]
fn scene_import_overlay_review_subframe_audio_survives() {
    // At 25 fps both edges round to zero, but 1ms of sample-accurate audio remains.
    let xml = scene(r#"<layer id="tiny" asset="v" start="0.001" end="0.002"><expression property="x">1</expression></layer>"#);
    let (p, _, _) = run(&xml);
    let audio: Vec<_> = seq(&p).audio_tracks.iter().flat_map(|t| &t.items).collect();
    assert_eq!(audio.len(), 1);
    assert_eq!((audio[0].start, audio[0].duration), (Tick(TICKS_PER_SECOND / 1000), Tick(TICKS_PER_SECOND / 1000)));
}

#[test]
fn scene_import_round3_repeat_audio_templates_are_not_occurrences() {
    // repeatType makes count occurrences; the resolver does not instantiate them.
    // Until it does, neither zero nor two copies may produce one phantom soundtrack.
    for count in [0, 2] {
        for three_d in [false, true] {
            let xml = scene(&format!(
                r#"<repeat id="r" count="{count}" timeStep="2"><group id="g"><layer id="a" asset="v" end="2" threeD="{three_d}"/></group></repeat>"#
            ));
            let (p, _, report) = run(&xml);
            assert_eq!(seq(&p).audio_tracks.iter().flat_map(|t| &t.items).count(), 0, "template is not an occurrence: count={count}");
            assert!(report.mentions("Audio in <repeat> is not mapped"), "report unavailable occurrence timing");
        }
    }
}

#[test]
fn scene_import_round3_three_d_video_audio_uses_source_clocks() {
    // child=2t-6: local [4,8) -> global [5,7); clipIn=2, speed=1 -> rate=2.
    let xml = scene(
        r#"<group id="g" isolate="true" start="2" end="10" timeOffset="2" timeScale="2"><layer id="a" asset="v" threeD="true" start="4" end="8" clipIn="2"/></group>"#,
    );
    let xml = xml.replace("duration=\"12\"", "duration=\"10\" background=\"#00000000\"");
    let (p, overlays, _) = run(&xml);
    assert_eq!(overlays.len(), 1);
    let audio: Vec<_> = seq(&p).audio_tracks.iter().flat_map(|t| &t.items).collect();
    assert_eq!(audio.len(), 1, "3D video must keep its soundtrack");
    assert_eq!((audio[0].start, audio[0].duration, audio[0].source_in, audio[0].speed), (sec(5), sec(2), sec(2), 2.));
    assert!(audio[0].link.is_none());
    assert_overlay_round_trip(&p);
}

#[test]
fn scene_import_round3_three_d_audio_loop_reverse_and_remap() {
    // Two 2s reverse plays, then a local remap from source 3 to 5 over 2s.
    for (attrs, child, expected) in [
        (r#"clipIn="2" clipOut="4" loop="1" reverse="true""#, "", vec![(sec(0), sec(2), Tick(sec(2).0 + 1), true), (sec(2), sec(2), Tick(sec(2).0 + 1), true)]),
        (r#"end="2" clipIn="8""#, r#"<timeRemap><key time="0" value="3"/><key time="2" value="5"/></timeRemap>"#, vec![(sec(0), sec(2), sec(3), false)]),
    ] {
        let (p, _, _) = run(&scene(&format!(r#"<layer id="a" asset="v" threeD="true" {attrs}>{child}</layer>"#)));
        let audio: Vec<_> = seq(&p).audio_tracks.iter().flat_map(|t| &t.items).map(|c| (c.start, c.duration, c.source_in, c.reverse)).collect();
        assert_eq!(audio, expected);
    }
}

#[test]
fn scene_import_round4_audio_after_three_d_sequence_child() {
    // sequenceType places each child after the preceding source span. clipOut=2
    // means a=[0,2), b=[2,4), even though a's visual class needs an overlay.
    let xml = scene(r#"<sequence id="sequence"><layer id="a" asset="v" threeD="true" clipOut="2"/><layer id="b" asset="v" clipOut="2"/></sequence>"#)
        .replace("duration=\"12\"", "duration=\"12\" background=\"#00000000\"");
    let (p, overlays, _) = run(&xml);
    let mut audio: Vec<_> = seq(&p).audio_tracks.iter().flat_map(|t| &t.items).collect();
    audio.sort_by_key(|c| c.start);
    assert_eq!(audio.len(), 2, "both source spans must keep their soundtrack");
    assert_eq!(
        audio.iter().map(|c| (c.name.as_str(), c.start, c.duration, c.source_in)).collect::<Vec<_>>(),
        vec![("a", sec(0), sec(2), sec(0)), ("b", sec(2), sec(2), sec(0))]
    );
    assert_eq!(overlays.len(), 1);
    assert_overlay_round_trip(&p);
}

#[test]
fn scene_import_round4_audio_animation_uses_corrected_sequence_clock() {
    // a occupies [0,2); b has a local volume ramp over [0,2], hence global
    // [2,4] and source [0,2]. Check the endpoints and the halfway gain (0.5).
    let xml = scene(
        r#"<sequence id="sequence"><layer id="a" asset="v" threeD="true" clipOut="2"/><layer id="b" asset="v" clipOut="2"><animate property="volume" timeBase="local"><key time="0" value="0"/><key time="2" value="1"/></animate></layer></sequence>"#,
    );
    let (p, _, _) = run(&xml);
    let b = seq(&p).audio_tracks.iter().flat_map(|t| &t.items).find(|c| c.name == "b").expect("ordinary audio after the 3D layer");
    assert_eq!((b.start, b.duration), (sec(2), sec(2)));
    let volume = b.effect("volume").unwrap().param("level").unwrap();
    assert_eq!(volume.keyframes.first().unwrap().time, sec(0));
    assert_eq!(volume.keyframes.last().unwrap().time, sec(2));
    assert!((volume.f64_at(sec(1)) - 20. * 0.5_f64.log10()).abs() < 1e-6);
}

#[test]
fn scene_import_round4_camera_without_target_shares_overlay() {
    // cameraType: the live camera selects projection implicitly; a camera-only
    // overlay cannot project the box in a different overlay.
    for nodes in [
        r#"<camera id="view" z="-1000" fov="20"/><object3D id="box" primitive="box"/>"#,
        r#"<object3D id="box" primitive="box"/><camera id="view" z="-1000" fov="20"/>"#,
    ] {
        let xml = scene(nodes).replace("duration=\"12\"", "duration=\"12\" background=\"#00000000\"");
        let (p, overlays, _) = run(&xml);
        assert_eq!(overlays.len(), 1, "camera and box must share one projection context");
        let doc = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
        for id in ["view", "box"] {
            assert_eq!(doc.doc.descendants().filter(|n| n.attribute("id") == Some(id)).count(), 1);
        }
        assert_eq!(seq(&p).video_tracks.iter().flat_map(|t| &t.items).count(), 1);
        assert_overlay_round_trip(&p);
    }
}

#[test]
fn scene_import_round4_camera_switch_preserves_scope_and_dependencies() {
    // cameraType picks the last active live camera: wide for [0,6), close for
    // [6,12). A group's camera still affects its sibling box outside that group.
    let xml = scene(r#"<group id="rig"><camera id="wide" z="-1000" fov="60"/><camera id="close" z="-1000" start="6" end="12" target="aim"><expression property="fov">param('lens')</expression><link property="x" source="aim.x"/></camera><shape id="aim" shape="rect" width="1" height="1" x="100"/></group><object3D id="box" primitive="box"/>"#)
        .replace("<assets>", r#"<parameters><param id="lens" type="number" default="20"/></parameters><assets>"#)
        .replace("duration=\"12\"", "duration=\"12\" background=\"#00000000\"");
    let (p, overlays, _) = run(&xml);
    assert_eq!(overlays.len(), 1, "switching cameras and their content must coalesce");
    let doc = read::read(std::str::from_utf8(&overlays[0].document).unwrap(), &Default::default()).unwrap();
    for id in ["rig", "wide", "close", "aim", "box", "lens"] {
        assert_eq!(doc.doc.descendants().filter(|n| n.attribute("id") == Some(id)).count(), 1, "{id}");
    }
    let cameras: Vec<_> = doc.doc.descendants().filter(|n| n.has_tag_name("camera")).collect();
    assert_eq!(cameras[0].attribute("id"), Some("wide"));
    assert_eq!((cameras[1].attribute("start"), cameras[1].attribute("end")), (Some("6"), Some("12")));
    assert!(doc.doc.descendants().any(|n| n.has_tag_name("expression") && n.text() == Some("param('lens')")));
    assert!(doc.doc.descendants().any(|n| n.has_tag_name("link") && n.attribute("source") == Some("aim.x")));
    assert_eq!(seq(&p).video_tracks.iter().flat_map(|t| &t.items).count(), 1);
    assert_overlay_round_trip(&p);
}

#[test]
fn scene_import_round4_audio_after_silent_three_d_sequence_child() {
    // A silent 3D source still advances the sequence cursor by its 2s source
    // span. The only soundtrack, b, must use that corrected cursor at 2s.
    let xml = scene(r#"<sequence id="s"><layer id="a" asset="silent" threeD="true" clipOut="2"/><layer id="b" asset="v" clipOut="2"/></sequence>"#)
        .replace("<assets>", r#"<assets><video id="silent" src="silent.mp4" width="640" height="480" fps="25" duration="10"/>"#);
    let (p, _, _) = run(&xml);
    let audio: Vec<_> = seq(&p).audio_tracks.iter().flat_map(|t| &t.items).collect();
    assert_eq!(audio.len(), 1, "silent preceding video must still advance the audio clock");
    assert_eq!((audio[0].name.as_str(), audio[0].start, audio[0].duration, audio[0].source_in), ("b", sec(2), sec(2), sec(0)));
}
