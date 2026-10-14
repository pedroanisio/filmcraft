//! Pure scene-to-project mapping and external-render overlay documents.

#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use super::{
    SceneDoc, err, read,
    resolve::{self, NodeClass, Resolved, ResolvedNode, SourceMap, SourceSegment},
    stack, validate,
    values::{self, Paint, Rgba},
};
use crate::common::{self, Builder, MediaSpec};
use crate::{Error, ImportOptions, Imported, Level, Report, ReportEntry, Result};
use filmcraft_media::{Generator, MediaKind};
use filmcraft_project::{ItemId, Sequence, TrackItem, TrackKind};
use filmcraft_time::{FrameRate, Tick};
use roxmltree::Node;
use std::collections::{BTreeMap, HashMap, HashSet};

mod graphic;
mod group;
mod meta;
mod overlay;
mod properties;
#[cfg(test)]
mod s6_tests;
#[cfg(test)]
mod tests;
mod transition;

const MAX_CLIPS: usize = 100_000;
const MAX_KEYS: usize = 100_000;
const MAX_OUTPUT: usize = 64 << 20;

/// A complete scene fragment for the engine to write before merging the imported project.
#[derive(Clone, Debug)]
pub struct Overlay {
    /// Fragment-local item referenced by the overlay clip.
    pub item: ItemId,
    /// Destination path, also stored in the item's file reference.
    pub path: String,
    /// Validated scene XML with transparent background.
    pub document: Vec<u8>,
}

#[derive(Default)]
struct Log {
    messages: BTreeMap<(Level, String), u32>,
    bytes: usize,
}
impl Log {
    fn add(&mut self, level: Level, message: impl Into<String>) -> Result<()> {
        let key = (level, message.into());
        if let Some(count) = self.messages.get_mut(&key) {
            *count = count.saturating_add(1);
        } else {
            self.bytes = self.bytes.saturating_add(key.1.len());
            if self.bytes > MAX_OUTPUT || self.messages.len() >= 100_000 {
                return Err(err("import report limit exceeded"));
            }
            self.messages.insert(key, 1);
        }
        Ok(())
    }
    fn warn(&mut self, message: impl Into<String>) -> Result<()> {
        self.add(Level::Warning, message)
    }
    fn finish(self) -> Report {
        Report { entries: self.messages.into_iter().map(|((level, message), count)| ReportEntry { level, message, count }).collect() }
    }
}

fn tick(seconds: f64) -> Result<Tick> {
    if !seconds.is_finite() || seconds.abs() >= Tick::MAX.0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64 {
        return Err(err("scene time is outside Tick range"));
    }
    values::seconds_to_tick(seconds).ok_or_else(|| err("scene time is outside Tick range"))
}
fn add(a: Tick, b: Tick) -> Result<Tick> {
    a.0.checked_add(b.0).map(Tick).ok_or_else(|| err("scene tick addition overflow"))
}
fn sub(a: Tick, b: Tick) -> Result<Tick> {
    a.0.checked_sub(b.0).map(Tick).ok_or_else(|| err("scene tick subtraction overflow"))
}
fn finite(v: f64) -> Result<f64> {
    if v.is_finite() { Ok(v) } else { Err(err("non-finite imported value")) }
}
fn flag(n: Node<'_, '_>, name: &str, default: bool) -> Result<bool> {
    n.attribute(name).map_or(Ok(default), |v| values::boolean(v).map_err(err))
}
fn integer(n: Node<'_, '_>, name: &str, default: u32) -> Result<u32> {
    let v = n.attribute(name).map_or(Ok(i64::from(default)), |v| values::integer_in(v, 1, i64::from(u32::MAX)).map_err(err))?;
    u32::try_from(v).map_err(|_| err("asset dimension out of range"))
}
fn xml_node<'a, 'i>(doc: &'a SceneDoc<'i>, n: &ResolvedNode) -> Result<Node<'a, 'i>> {
    doc.doc.get_node(n.xml).ok_or_else(|| err("resolved XML node is missing"))
}

#[derive(Default)]
struct Snaps {
    count: u32,
    max: i128,
}
impl Snaps {
    fn edge(&mut self, t: Tick, fps: FrameRate) -> Result<Tick> {
        let frame = fps.frame_at(t);
        let lo = fps.tick_of(frame);
        let hi = fps.tick_of(frame.checked_add(1).ok_or_else(|| err("frame index overflow"))?);
        let down = i128::from(t.0) - i128::from(lo.0);
        let up = i128::from(hi.0) - i128::from(t.0);
        let snapped = if down < up || (down == up && frame % 2 == 0) { lo } else { hi };
        if snapped != t {
            self.count = self.count.saturating_add(1);
            self.max = self.max.max((i128::from(snapped.0) - i128::from(t.0)).abs());
        }
        Ok(snapped)
    }
    fn finish(self, log: &mut Log) -> Result<()> {
        if self.count > 0 {
            log.messages.insert((Level::Info, "Video edge snapped to frame grid".into()), self.count);
            log.add(Level::Info, format!("Maximum video edge snap error: {} ticks", self.max))?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Asset {
    item: ItemId,
    video: bool,
    audio: bool,
    audio_stream: usize,
}

/// Validate, resolve and map a scene, returning any externally rendered subdocuments.
/// Malformed XML, unsafe numbers and resource limits fail even in lenient mode.
pub fn import(bytes: &[u8], opts: &ImportOptions) -> Result<(Imported, Vec<Overlay>, Report)> {
    let limits = read::Limits::default();
    let text = read::decode(bytes, &limits)?;
    let doc = read::read(text, &limits)?;
    let diags = validate::validate(&doc.doc);
    if !opts.lenient && diags.iter().any(|d| d.is_error()) {
        return Err(Error::Invalid(diags));
    }
    if diags.iter().any(|d| d.code == "W01") {
        return Err(err("non-finite numeric input is not permitted, including in lenient imports"));
    }
    if doc.doc.descendants().any(|n| n.attribute("id").is_some_and(|id| id.len() > 4096)) {
        return Err(err("import identifier length exceeds 4096 bytes"));
    }
    let mut log = Log::default();
    for d in diags {
        log.warn(d.to_string())?;
    }
    let resolved = resolve::resolve(&doc, &Default::default())?;
    for note in &resolved.notes {
        log.warn(format!("{}: {}", note.code, note.message))?;
    }
    if resolved.nodes.len().saturating_mul(resolved.transitions.len()) > 2_000_000 {
        return Err(err("transition occupancy mapping work limit exceeded"));
    }
    let name = opts.name.as_deref().map(common::file_stem).unwrap_or("Imported Scene");
    let mut b = Builder::new(name);
    let p = &resolved.project;
    let settings = common::settings_for(p.fps, p.width, p.height, false);
    let mut seq = common::empty_sequence(settings.clone());
    let sequence = b.reserve_sequence(name, settings, None);
    let assets = assets(&doc, &resolved, opts, &mut b, &mut log)?;
    let audio_resolved = resolve_audio_layers(&doc, &resolved, &assets)?;
    if let Some(audio) = &audio_resolved {
        for note in &audio.notes {
            if note.node.and_then(|i| resolved.node(i)).is_some_and(|n| n.source.is_none()) {
                log.warn(format!("{}: {}", note.code, note.message))?;
            }
        }
    }
    let duration = tick(p.duration)?;
    let mut video = Vec::new();
    let mut audio = Vec::new();
    let mut snaps = Snaps::default();
    if let Some(color) = background(&doc, &p.background, &mut log)? {
        let spec = MediaSpec { duration: Some(duration), video: Some((p.width, p.height, p.fps)), ..Default::default() };
        let item = b.generator_media("@background", "Scene Background", Generator::ColorMatte { color }, &spec, None);
        let end = snaps.edge(duration, p.fps)?;
        if end > Tick::ZERO {
            video.push(b.clip(item, TrackKind::Video, "Scene Background", Tick::ZERO, end, Tick::ZERO));
        } else {
            log.warn("Video clip dropped: zero frames")?;
        }
    }
    let mut overlays = Vec::new();
    let mut roots: HashSet<_> = resolved.unmappable.iter().map(|n| n.node).collect();
    roots.extend(resolved.nodes.iter().filter(|n| n.unsupported.iter().any(|f| f == "expression")).map(|n| n.index));
    group::overlay_roots(&resolved, &mut roots, &mut log)?;
    if roots.len() > 512 || roots.len().saturating_mul(doc.doc.descendants().count()) > 2_000_000 {
        return Err(err("overlay generation work limit exceeded"));
    }
    let plans = overlay::prepare(&doc, &resolved, &roots, opts)?;
    let mut output_bytes = plans.iter().fold(0usize, |bytes, p| bytes.saturating_add(p.document.len()));
    let covered: HashSet<_> = plans.iter().flat_map(|p| p.covered.iter().copied()).collect();
    let whole_composition = plans.iter().any(|p| p.root.is_none());
    let mut plans: HashMap<_, _> = plans.into_iter().map(|p| (p.anchor, p)).collect();
    if resolved.nodes.iter().filter(|n| n.class == NodeClass::Text).count().saturating_mul(doc.doc.descendants().count()) > 2_000_000 {
        return Err(err("text asset mapping work limit exceeded"));
    }
    let mut keys = 0usize;
    let mut sampled_keys = 0usize;
    for n in &resolved.nodes {
        if let Some(plan) = plans.remove(&n.index) {
            let root = plan.root.and_then(|i| resolved.node(i));
            let interval = root.map_or(resolve::Interval { start: 0., end: p.duration }, |n| n.interval);
            let start = snaps.edge(tick(interval.start)?, p.fps)?;
            let end = snaps.edge(tick(interval.end)?, p.fps)?;
            if end <= start {
                log.warn("Video clip dropped: zero frames")?;
            } else {
                let name = root.map_or("Scene Composition", |n| n.id.as_str());
                let overlay::Plan { path, document, has_audio, .. } = plan;
                let item = b.file_media(
                    &format!("@overlay-{}", n.index),
                    name,
                    &path,
                    &MediaSpec { duration: Some(duration), video: Some((p.width, p.height, p.fps)), ..Default::default() },
                    None,
                );
                if let Some(m) = b.p.item_mut(item).and_then(|i| i.as_media_mut()).and_then(|m| m.info.video.as_mut()) {
                    m.has_alpha = true;
                }
                let mut clip = b.clip(item, TrackKind::Video, name, start, sub(end, start)?, start);
                clip.enabled = root.is_none_or(|n| n.visible);
                video.push(clip);
                log.warn(format!("Unmapped <{}>: preserved in scene overlay", n.element))?;
                if has_audio {
                    log.warn("Scene overlay contains audio; native media audio is mapped separately, but generated or symbol audio is not mixed")?;
                }
                overlays.push(Overlay { item, path, document });
            }
        }
        let covered_video = covered.contains(&n.index);
        let xml = xml_node(&doc, n)?;
        let media_asset = xml.has_tag_name("layer").then(|| xml.attribute("asset").and_then(|id| assets.get(id))).flatten();
        if matches!(n.class, NodeClass::Media(_)) || media_asset.is_some_and(|a| a.audio) {
            let asset = media_asset.ok_or_else(|| err("layer asset is missing"))?;
            if xml.ancestors().skip(1).any(|p| p.has_tag_name("repeat")) {
                // The resolver visits repeat templates, not the renderer's expanded
                // occurrences. A template has neither a guaranteed count nor a clock.
                if asset.audio {
                    log.warn("Audio in <repeat> is not mapped: expanded occurrence timing is unavailable; template soundtrack omitted")?;
                }
                continue;
            }
            // A recovered source span can shift every later sequence child,
            // including layers that already had a source map in the visual pass.
            let mapping = if asset.audio { audio_resolved.as_ref().unwrap_or(&resolved) } else { &resolved };
            let n = mapping.node(n.index).filter(|m| m.xml == n.xml && m.id == n.id).ok_or_else(|| err("audio node identity changed during resolution"))?;
            if covered_video && !asset.audio {
                continue;
            }
            let segments = segments(mapping, n, &mut log)?;
            let channels = properties::channels(&doc, mapping, n, &mut log, &mut sampled_keys)?;
            let (occupied_start, occupied_end) = transition::occupancy(mapping, n)?;
            for s in segments {
                if video.len().saturating_add(audio.len()).saturating_add(2) > MAX_CLIPS {
                    return Err(err("import clip limit exceeded"));
                }
                let segment_start = tick(s.comp_start)?;
                let raw_start = segment_start.max(occupied_start);
                let raw_end = tick(s.comp_end)?.min(occupied_end);
                if raw_end <= raw_start {
                    continue;
                }
                let source = tick(s.src_start)?;
                tick(finite(s.src_start + finite((s.comp_end - s.comp_start) * s.rate)?)?)?;
                let link = (asset.video && asset.audio && !covered_video).then(|| b.link_id());
                for (kind, enabled) in [(TrackKind::Video, asset.video && !covered_video), (TrackKind::Audio, asset.audio)] {
                    if !enabled {
                        continue;
                    }
                    let (start, end) =
                        if kind == TrackKind::Video { (snaps.edge(raw_start, p.fps)?, snaps.edge(raw_end, p.fps)?) } else { (raw_start, raw_end) };
                    if end <= start {
                        log.warn(if kind == TrackKind::Video { "Video clip dropped: zero frames" } else { "Audio clip dropped: empty span" })?;
                        continue;
                    }
                    let phase = scaled_ticks(sub(start, segment_start)?, s.rate.abs())?;
                    let source = if s.rate < 0. { sub(source, phase)? } else { add(source, phase)? };
                    let consumed = scaled_ticks(sub(end, start)?, s.rate.abs())?;
                    let source_in =
                        if s.rate < 0. { Tick(sub(source, consumed)?.0.checked_add(1).ok_or_else(|| err("reverse source overflow"))?) } else { source };
                    source_in.0.checked_add(consumed.0).ok_or_else(|| err("source out overflow"))?;
                    let mut c = b.clip(asset.item, kind, &n.id, start, sub(end, start)?, source_in);
                    c.speed = finite(s.rate.abs())?;
                    c.reverse = s.rate < 0.;
                    c.link = link;
                    c.enabled = n.visible;
                    if s.rate == 0. {
                        c.frame_hold = Some(source);
                        c.hold_filters = false;
                        c.speed = 1.;
                    }
                    c.audio_stream = asset.audio_stream;
                    properties::apply(&mut c, kind, n, &channels, &mut keys, &mut log)?;
                    if kind == TrackKind::Video {
                        group::relative_opacity(&mut c, &resolved, n)?;
                        video.push(c);
                    } else {
                        audio.push(c);
                    }
                }
            }
        } else if covered_video {
            continue;
        } else if matches!(n.class, NodeClass::Text | NodeClass::Shape(_)) {
            if let Some(c) = graphic::clip(&doc, &resolved, n, &mut b, &mut log, &mut snaps)? {
                if video.len().saturating_add(audio.len()) >= MAX_CLIPS {
                    return Err(err("import clip limit exceeded"));
                }
                for e in &c.effects {
                    for param in e.params.values() {
                        keys = keys.saturating_add(param.keyframes.len());
                        output_bytes = output_bytes.saturating_add(match &param.value {
                            filmcraft_project::ParamValue::Text(s) => s.len(),
                            filmcraft_project::ParamValue::Curve(v) => v.len().saturating_mul(8),
                            _ => 0,
                        });
                    }
                }
                if keys > MAX_KEYS || output_bytes > MAX_OUTPUT {
                    return Err(err("graphic import output limit exceeded"));
                }
                video.push(c);
            }
        } else {
            match n.class {
                NodeClass::Group { .. } | NodeClass::Sequence | NodeClass::Transition => {}
                _ => log.warn(format!("Unmapped <{}>", n.element))?,
            }
        }
    }
    report_unmapped(&doc, &resolved, &mut log)?;
    if !whole_composition {
        group::fold(&doc, &resolved, &mut b, &mut video, &mut audio, &mut log, &mut snaps)?;
    }
    transition::place(&resolved, &mut b, &mut seq, video, &mut log)?;
    place(&mut b, &mut seq, TrackKind::Audio, audio)?;
    transition::apply(&doc, &resolved, &mut b, &mut seq, &mut log, &mut snaps, None)?;
    meta::apply(&doc, &resolved, &assets, &mut b, &mut seq, &mut log)?;
    if seq.duration() < duration {
        // Sequence has no independent duration field. A disabled carrier preserves the empty tail.
        let start = seq.duration();
        let item = b.generator_media(
            "@duration",
            "Scene Duration",
            Generator::TransparentVideo,
            &MediaSpec { duration: Some(duration), video: Some((p.width, p.height, p.fps)), ..Default::default() },
            None,
        );
        let mut c = b.clip(item, TrackKind::Video, "Scene Duration", start, sub(duration, start)?, Tick::ZERO);
        c.enabled = false;
        b.ensure_tracks(&mut seq, TrackKind::Video, 1);
        seq.video_tracks.first_mut().ok_or_else(|| err("missing duration track"))?.items.push(c);
        log.add(Level::Info, "Trailing blank time retained with a disabled duration carrier")?;
    }
    seq.mark_out = Some(duration);
    b.put_sequence(sequence, seq);
    snaps.finish(&mut log)?;
    fill_unknown_durations(&mut b.p)?;
    Ok((Imported { project: b.p, sequences: vec![sequence] }, overlays, log.finish()))
}

// Reuse the bounded semantic pass for soundtracks whose visuals are 3D. Changing
// only attribute values preserves XML node identities, clocks and source controls.
// This runs at most once per import; the original document still drives overlays.
fn resolve_audio_layers(doc: &SceneDoc<'_>, resolved: &Resolved, assets: &HashMap<String, Asset>) -> Result<Option<Resolved>> {
    let mut has_audio = false;
    let mut needs_timing = false;
    for n in &resolved.nodes {
        let xml = xml_node(doc, n)?;
        if !xml.has_tag_name("layer") || xml.ancestors().any(|p| p.has_tag_name("repeat")) {
            continue;
        }
        if let Some(asset) = xml.attribute("asset").and_then(|id| assets.get(id)) {
            has_audio |= asset.audio;
            // Silent media also advance the sequence cursor before later audio.
            needs_timing |= n.source.is_none();
        }
    }
    if !has_audio || !needs_timing {
        return Ok(None);
    }
    let input = doc.doc.input_text();
    let mut output = String::new();
    let mut cursor = 0usize;
    for xml in doc.doc.descendants().filter(|n| n.has_tag_name("layer")) {
        if !xml.attribute("asset").is_some_and(|id| assets.contains_key(id)) || !flag(xml, "threeD", false)? {
            continue;
        }
        let range = xml.attribute_node("threeD").ok_or_else(|| err("missing 3D attribute"))?.range_value();
        output.push_str(input.get(cursor..range.start).ok_or_else(|| err("invalid audio attribute range"))?);
        output.push('0');
        cursor = range.end;
    }
    output.push_str(input.get(cursor..).ok_or_else(|| err("invalid audio document range"))?);
    // Each replacement shrinks its value; the copy is capped by the input budget.
    let audio_doc = read::read(&output, &Default::default())?;
    resolve::resolve(&audio_doc, &Default::default()).map(Some)
}

fn fill_unknown_durations(project: &mut filmcraft_project::Project) -> Result<()> {
    let mut bounds: HashMap<ItemId, Tick> = HashMap::new();
    for item in project.items.values() {
        if let Some(seq) = item.as_sequence() {
            for clip in seq.all_tracks().flat_map(|t| &t.items) {
                let source_out = add(clip.source_in, scaled_ticks(clip.duration, clip.speed.abs())?)?;
                let end = source_out.max(add(clip.frame_hold.unwrap_or(clip.source_in), Tick(1))?);
                let bound = bounds.entry(clip.item).or_insert(Tick::ZERO);
                *bound = (*bound).max(end);
            }
        }
    }
    for (item, end) in bounds {
        if let Some(media) = project.item_mut(item).and_then(|i| i.as_media_mut())
            && media.info.duration == Tick::ZERO
        {
            media.info.duration = end;
        }
    }
    Ok(())
}

fn place(b: &mut Builder, seq: &mut Sequence, kind: TrackKind, clips: Vec<TrackItem>) -> Result<()> {
    let spans: Vec<_> = clips.iter().map(|c| stack::Span { start: c.start, end: c.end() }).collect();
    let tracks = stack::assign(&spans);
    for (c, t) in clips.into_iter().zip(tracks) {
        let index = usize::try_from(t).map_err(|_| err("track index overflow"))?;
        if index >= MAX_CLIPS {
            return Err(err("track limit exceeded"));
        }
        b.ensure_tracks(seq, kind, index.saturating_add(1));
        seq.tracks_mut(kind).get_mut(index).ok_or_else(|| err("missing assigned track"))?.items.push(c);
    }
    Ok(())
}

fn assets(doc: &SceneDoc<'_>, resolved: &Resolved, opts: &ImportOptions, b: &mut Builder, log: &mut Log) -> Result<HashMap<String, Asset>> {
    let mut out = HashMap::new();
    let Some(container) = doc.root().children().find(|n| n.has_tag_name("assets")) else { return Ok(out) };
    for n in container.children().filter(Node::is_element) {
        let name = n.tag_name().name();
        if name == "text" {
            continue;
        }
        if !matches!(name, "video" | "image" | "audio" | "imageSequence") {
            log.warn(format!("Unmapped asset <{name}>"))?;
            continue;
        }
        let id = n.attribute("id").ok_or_else(|| err("asset needs id"))?;
        let video = name != "audio";
        let audio = name == "audio" || flag(n, "hasAudio", false)?;
        let fps = n.attribute("fps").map_or(Ok(resolved.project.fps), |v| values::fps(v).map_err(err))?;
        let duration = if name == "imageSequence" {
            let first = values::integer_in(n.attribute("first").ok_or_else(|| err("imageSequence needs first"))?, i64::MIN, i64::MAX).map_err(err)?;
            let last = values::integer_in(n.attribute("last").ok_or_else(|| err("imageSequence needs last"))?, i64::MIN, i64::MAX).map_err(err)?;
            let step = values::integer_in(n.attribute("step").unwrap_or("1"), 1, i64::MAX).map_err(err)?;
            let count = last
                .checked_sub(first)
                .and_then(|v| v.checked_div(step))
                .and_then(|v| v.checked_add(1))
                .filter(|v| *v > 0)
                .ok_or_else(|| err("imageSequence frame count overflow"))?;
            Some(tick(count as f64 * fps.den as f64 / fps.num as f64)?)
        } else {
            n.attribute("duration").map(|v| values::seconds(v).map_err(err)).transpose()?
        };
        let spec = MediaSpec {
            duration,
            video: if video { Some((integer(n, "width", resolved.project.width)?, integer(n, "height", resolved.project.height)?, fps)) } else { None },
            audio: if audio { Some((integer(n, "sampleRate", 48000)?, integer(n, "channels", 2)?)) } else { None },
            kind: Some(match name {
                "audio" => MediaKind::AudioOnly,
                "image" => MediaKind::Still,
                _ => MediaKind::Movie,
            }),
            ..Default::default()
        };
        if audio && (n.attribute("sampleRate").is_none() || n.attribute("channels").is_none()) {
            log.add(Level::Info, "Audio metadata absent: assumed 48000 Hz / stereo where unspecified")?;
        }
        let src = n.attribute("src").ok_or_else(|| err("asset needs src"))?;
        let path = safe_path(src, opts.base_dir.as_deref())?;
        let item = b.file_media(id, id, &path, &spec, None);
        if let Some(m) = b.p.item_mut(item).and_then(|i| i.as_media_mut()) {
            if let Some(v) = m.info.video.as_mut() {
                v.has_alpha = match n.attribute("alpha") {
                    Some("none") => false,
                    Some("straight" | "premultiplied") => true,
                    _ => name != "video",
                };
            }
            m.interpret.ignore_alpha = n.attribute("alpha") == Some("none");
        }
        if name == "imageSequence" {
            log.warn("Unmapped imageSequence file enumeration: retained numbered source pattern; engine must relink")?;
        }
        let audio_stream = n.attribute("audioStream").map_or(Ok(0), |s| values::integer_in(s, 0, 1024).map_err(err))?;
        out.insert(id.into(), Asset { item, video, audio, audio_stream: usize::try_from(audio_stream).map_err(|_| err("audio stream overflow"))? });
    }
    Ok(out)
}

fn safe_path(src: &str, base: Option<&str>) -> Result<String> {
    // common::file_url_to_path slices the host at byte 9; guard its Unicode boundary.
    if src.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("file://")) {
        let rest = src.get(7..).ok_or_else(|| err("invalid file URL"))?;
        if rest.len() >= 9 && !rest.is_char_boundary(9) {
            return Err(err("file URL host has an unsupported Unicode boundary"));
        }
    }
    Ok(common::resolve_path(src, base))
}

fn background(doc: &SceneDoc<'_>, paint: &Paint, log: &mut Log) -> Result<Option<[f32; 4]>> {
    let color = match paint {
        Paint::Color(c) => *c,
        Paint::Ref(id) => {
            log.warn("Unmapped gradient background: using first stop colour")?;
            let stop = doc.doc.descendants().find(|n| n.attribute("id") == Some(id)).and_then(|n| n.children().find(|n| n.has_tag_name("stop")));
            if let Some(stop) = stop {
                let Paint::Color(Rgba([r, g, b, a])) = values::color(stop.attribute("color").unwrap_or("#000000FF")).map_err(err)? else {
                    return Err(err("gradient stop needs solid color"));
                };
                Rgba([r, g, b, a * values::unit(stop.attribute("opacity").unwrap_or("1")).map_err(err)? as f32])
            } else {
                log.warn("Background paint has no solid first stop; using opaque black")?;
                Rgba::BLACK
            }
        }
        Paint::Var(_) => return Err(err("unresolved background token")),
    };
    let Rgba([_, _, _, alpha]) = color;
    Ok(if alpha == 0. { None } else { Some(color.0) })
}

fn segments(resolved: &Resolved, n: &ResolvedNode, log: &mut Log) -> Result<Vec<SourceSegment>> {
    match n.source.as_ref() {
        Some(SourceMap::Segments(s)) => Ok(s.clone()),
        Some(SourceMap::Remap { keys, freeze_local }) => {
            log.warn("Unmapped timeRemap absolute source curve: retaining first segment behaviour")?;
            let mut first = keys.first().ok_or_else(|| err("empty source remap"))?;
            if let Some(local) = freeze_local {
                let time = resolved.key_time(n.index, resolve::TimeBase::Local, *local).ok_or_else(|| err("invalid remap freeze time"))?;
                let mut next = None;
                for key in keys {
                    if key.comp_time <= time {
                        first = key;
                    } else {
                        next = Some(key);
                        break;
                    }
                }
                let source = if time < first.comp_time || first.interpolation == "hold" {
                    first.source_time
                } else if let Some(next) = next {
                    let width = next.comp_time - first.comp_time;
                    if width > 0. {
                        finite(first.source_time + (next.source_time - first.source_time) * ((time - first.comp_time) / width))?
                    } else {
                        first.source_time
                    }
                } else {
                    first.source_time
                };
                return Ok(vec![SourceSegment { comp_start: n.span.start, comp_end: n.span.end, src_start: source, rate: 0. }]);
            }
            let rate = if first.interpolation == "hold" {
                0.
            } else if let Some(next) = keys.get(1) {
                let delta = next.comp_time - first.comp_time;
                if delta > 0. { finite((next.source_time - first.source_time) / delta)? } else { 0. }
            } else {
                0.
            };
            let source = finite(first.source_time + finite((n.span.start - first.comp_time) * rate)?)?;
            Ok(vec![SourceSegment { comp_start: n.span.start, comp_end: n.span.end, src_start: source, rate }])
        }
        None => Err(err("media node has no source map")),
    }
}

fn report_unmapped(doc: &SceneDoc<'_>, resolved: &Resolved, log: &mut Log) -> Result<()> {
    let nodes: HashSet<_> = resolved.nodes.iter().map(|n| n.xml).collect();
    for n in &resolved.nodes {
        for feature in &n.unsupported {
            if !doc.doc.get_node(n.xml).is_some_and(|xml| xml.descendants().skip(1).any(|c| c.has_tag_name(feature.as_str()))) {
                log.warn(format!("Unmapped <{feature}>"))?;
            }
        }
        if let Some(layout) = &n.layout {
            if layout.transform.skew_x != 0. || layout.transform.skew_y != 0. {
                log.warn("Unmapped skew transform")?;
            }
            if layout.parent_transform_unresolved {
                log.warn("Unmapped animated parent transform; local Motion retained")?;
            } else if !layout.transform_static
                && n.transform_parent
                    .and_then(|i| resolved.node(i))
                    .and_then(|p| p.layout.as_ref())
                    .and_then(|l| l.world_matrix)
                    .is_some_and(|m| [m.a - 1., m.b, m.c, m.d - 1., m.e, m.f].iter().any(|v| v.abs() > 1e-9))
            {
                log.warn("Unmapped parent transform on animated Motion; resolver has no static world matrix, local Motion retained")?;
            }
        }
    }
    for n in doc.doc.descendants().filter(Node::is_element) {
        report_attributes(n, log)?;
        if nodes.contains(&n.id()) {
            continue;
        }
        let name = n.tag_name().name();
        if matches!(
            name,
            "scene"
                | "project"
                | "assets"
                | "composition"
                | "video"
                | "image"
                | "audio"
                | "imageSequence"
                | "animate"
                | "key"
                | "timeRemap"
                | "text"
                | "markers"
                | "marker"
                | "captions"
                | "captionTrack"
                | "cue"
                | "audioMix"
                | "audioTrack"
                | "master"
        ) {
            continue;
        }
        log.warn(format!("Unmapped <{name}>"))?;
    }
    Ok(())
}

fn scaled_ticks(value: Tick, rate: f64) -> Result<Tick> {
    finite(rate)?;
    if rate < 0. {
        return Err(err("negative magnitude"));
    }
    if rate == 0. {
        return Ok(Tick::ZERO);
    }
    let bits = rate.to_bits();
    let exponent = i32::try_from((bits >> 52) & 0x7ff).map_err(|_| err("rate exponent"))?;
    let mantissa = (bits & ((1u64 << 52) - 1)) | if exponent == 0 { 0 } else { 1u64 << 52 };
    let power = if exponent == 0 { -1074 } else { exponent - 1023 - 52 };
    let product = i128::from(value.0).checked_mul(i128::from(mantissa)).ok_or_else(|| err("speed multiplication overflow"))?;
    let result = if power >= 0 {
        let factor = 1i128
            .checked_shl(u32::try_from(power).map_err(|_| err("speed exponent overflow"))?)
            .filter(|v| *v > 0)
            .ok_or_else(|| err("speed exceeds Tick range"))?;
        product.checked_mul(factor).ok_or_else(|| err("speed exceeds Tick range"))?
    } else {
        let shift = power.unsigned_abs();
        if shift >= 127 {
            0
        } else {
            let divisor = 1i128.checked_shl(shift).ok_or_else(|| err("speed divisor overflow"))?;
            let q = product.div_euclid(divisor);
            let rem = product.rem_euclid(divisor);
            q + i128::from(rem > divisor / 2 || (rem == divisor / 2 && q % 2 != 0))
        }
    };
    i64::try_from(result).map(Tick).map_err(|_| err("speed exceeds Tick range"))
}

pub(crate) fn drop_overlays(imported: &mut Imported, overlays: &[Overlay]) -> Result<()> {
    let ids: HashSet<_> = overlays.iter().map(|o| o.item).collect();
    let sequences: Vec<_> = imported.project.items.values().filter(|i| i.as_sequence().is_some()).map(|i| i.id).collect();
    for id in &sequences {
        let Some(seq) = imported.project.sequence_mut(*id) else {
            continue;
        };
        for track in seq.all_tracks_mut() {
            let removed: HashSet<_> = track.items.iter().filter(|c| ids.contains(&c.item)).map(|c| c.id).collect();
            track.items.retain(|c| !ids.contains(&c.item));
            track.transitions.retain(|tr| !tr.from.into_iter().chain(tr.to).any(|id| removed.contains(&id)));
        }
    }
    for id in ids {
        imported.project.items.remove(&id);
        imported.project.root.remove_item(id);
    }
    for id in sequences {
        let Some(seq) = imported.project.sequence(id) else {
            continue;
        };
        let duration = seq.mark_out.unwrap_or(Tick::ZERO);
        let start = seq.duration();
        if start >= duration {
            continue;
        }
        let settings = seq.settings.clone();
        let spec = MediaSpec {
            duration: Some(duration),
            video: Some((settings.width, settings.height, settings.frame_rate)),
            kind: Some(MediaKind::Synthetic),
            ..Default::default()
        };
        let media = common::media_clip(filmcraft_project::MediaRef::Generator(Generator::TransparentVideo), common::media_info("Scene Duration", &spec));
        let item = imported.project.add_item("Scene Duration", filmcraft_project::Label::Lavender, filmcraft_project::ItemKind::Media(media), None);
        let source = filmcraft_time::TimeRange::new(Tick::ZERO, sub(duration, start)?);
        let mut clip =
            imported.project.make_track_item(item, TrackKind::Video, start, source, settings.frame_rate).ok_or_else(|| err("cannot retain scene duration"))?;
        clip.enabled = false;
        let track_id = filmcraft_project::TrackId(imported.project.alloc_id());
        let seq = imported.project.sequence_mut(id).ok_or_else(|| err("missing scene sequence"))?;
        if seq.video_tracks.is_empty() {
            seq.video_tracks.push(filmcraft_project::Track::new(track_id, TrackKind::Video, "Scene Duration".into()));
        }
        seq.video_tracks.first_mut().ok_or_else(|| err("missing duration track"))?.items.push(clip);
    }
    Ok(())
}

fn report_attributes(n: Node<'_, '_>, log: &mut Log) -> Result<()> {
    let name = n.tag_name().name();
    if !matches!(name, "scene" | "project" | "layer" | "group" | "sequence" | "video" | "image" | "audio" | "imageSequence") {
        return Ok(());
    }
    for a in n.attributes() {
        let handled = match name {
            "scene" => a.name() == "version",
            "project" => matches!(a.name(), "width" | "height" | "fps" | "duration" | "background"),
            "video" | "image" | "audio" | "imageSequence" => matches!(
                a.name(),
                "id" | "src"
                    | "width"
                    | "height"
                    | "fps"
                    | "duration"
                    | "sampleRate"
                    | "channels"
                    | "hasAudio"
                    | "audioStream"
                    | "alpha"
                    | "first"
                    | "last"
                    | "step"
            ),
            _ => mapped_node_attribute(a.name()),
        };
        if !handled {
            log.warn(format!("Unmapped {name}@{}", a.name()))?;
        }
    }
    Ok(())
}

fn mapped_node_attribute(name: &str) -> bool {
    matches!(
        name,
        "id" | "asset"
            | "start"
            | "end"
            | "startMarker"
            | "endMarker"
            | "clipIn"
            | "clipOut"
            | "speed"
            | "timeStretch"
            | "loop"
            | "reverse"
            | "freezeAt"
            | "parent"
            | "x"
            | "y"
            | "anchorX"
            | "anchorY"
            | "scaleX"
            | "scaleY"
            | "rotation"
            | "opacity"
            | "visible"
            | "z"
            | "volume"
            | "mute"
            | "width"
            | "height"
            | "fit"
            | "boxWidth"
            | "boxHeight"
            | "alignX"
            | "alignY"
            | "alignTo"
            | "margin"
            | "timeOffset"
            | "timeScale"
            | "timeGap"
            | "isolate"
            | "name"
            | "threeD"
    )
}
