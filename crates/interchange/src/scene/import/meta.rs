use super::*;
use filmcraft_project::{Caption, CaptionFormat, CaptionTrack, ClipId, Label, Marker, MarkerId, MarkerKind, TrackId};

fn number(n: Node<'_, '_>, name: &str, default: f64) -> Result<f64> {
    n.attribute(name).map_or(Ok(default), |s| values::double(s).map_err(err))
}
fn rest(n: Node<'_, '_>, handled: &[&str], log: &mut Log) -> Result<()> {
    for child in n.children().filter(|n| n.has_tag_name("animate")) {
        log.warn(format!("Unmapped audio automation {} on <{}>", child.attribute("property").unwrap_or(""), n.tag_name().name()))?;
    }
    for a in n.attributes() {
        if !handled.contains(&a.name()) {
            log.warn(format!("Unmapped {}@{}", n.tag_name().name(), a.name()))?;
        }
    }
    Ok(())
}
pub(super) fn apply(doc: &SceneDoc<'_>, r: &Resolved, assets: &HashMap<String, Asset>, b: &mut Builder, seq: &mut Sequence, log: &mut Log) -> Result<()> {
    let mut count = 0usize;
    let mut bytes = 0usize;
    let mut audio_tracks = 0usize;
    for n in doc.doc.descendants().filter(Node::is_element) {
        if matches!(n.tag_name().name(), "marker" | "cue" | "captionTrack" | "audioTrack") {
            count = count.saturating_add(1);
        }
        if n.has_tag_name("captionTrack") {
            let cues = n.children().filter(|c| c.has_tag_name("cue")).count();
            let label = n.attribute("label").or_else(|| n.attribute("id")).unwrap_or("").len();
            bytes = bytes.saturating_add(cues.saturating_mul(label.saturating_add(n.attribute("language").unwrap_or("").len())));
        }
        if n.has_tag_name("audioTrack") {
            audio_tracks = audio_tracks.saturating_add(1);
            let plays = values::integer_in(n.attribute("loop").unwrap_or("0"), 0, 100_000).map_err(err)?.saturating_add(1);
            bytes = bytes.saturating_add(usize::try_from(plays).map_err(|_| err("audio loop count"))?.saturating_mul(n.attribute("id").unwrap_or("").len()));
        }
    }
    if count > MAX_CLIPS || bytes > MAX_OUTPUT || audio_tracks.saturating_mul(doc.doc.descendants().count()) > 2_000_000 {
        return Err(err("metadata mapping resource limit exceeded"));
    }
    let mut lane_work = 0usize;
    for container in doc.root().children().filter(Node::is_element) {
        match container.tag_name().name() {
            "markers" => {
                for n in container.children().filter(|n| n.has_tag_name("marker")) {
                    let start = tick(number(n, "time", 0.)?)?;
                    let duration = tick(number(n, "duration", 0.)?)?;
                    add(start, duration)?;
                    if duration < Tick::ZERO {
                        return Err(err("negative marker duration"));
                    }
                    let mut color = Label::Green;
                    if let Some(raw) = n.attribute("color") {
                        let Paint::Color(Rgba([red, green, blue, alpha])) = values::color(raw).map_err(err)? else {
                            return Err(err("marker needs solid colour"));
                        };
                        let mut best = f64::MAX;
                        for candidate in Label::ALL {
                            let [cr, cg, cb] = candidate.marker_rgb();
                            let distance = (f64::from(red) * 255. - f64::from(cr)).powi(2)
                                + (f64::from(green) * 255. - f64::from(cg)).powi(2)
                                + (f64::from(blue) * 255. - f64::from(cb)).powi(2);
                            if distance < best {
                                best = distance;
                                color = candidate;
                            }
                        }
                        if best > 0.01 || alpha != 1. {
                            log.warn("Marker colour approximated by nearest FilmCraft label")?;
                        }
                    }
                    let kind = match n.attribute("kind").unwrap_or("cue") {
                        "chapter" => MarkerKind::Chapter,
                        "section" => MarkerKind::Segmentation,
                        "cue" | "comment" => MarkerKind::Comment,
                        other => {
                            log.warn(format!("Unmapped marker kind {other}; comment marker retained"))?;
                            MarkerKind::Comment
                        }
                    };
                    seq.markers.push(Marker {
                        id: MarkerId(b.alloc()),
                        start,
                        duration,
                        name: n.attribute("label").or_else(|| n.attribute("id")).unwrap_or("").into(),
                        comment: n.attribute("comment").unwrap_or("").into(),
                        kind,
                        color,
                    });
                    rest(n, &["id", "time", "duration", "label", "kind", "color", "comment"], log)?;
                }
            }
            "captions" => {
                for n in container.children().filter(|n| n.has_tag_name("captionTrack")) {
                    let mut track = CaptionTrack::new(
                        TrackId(b.alloc()),
                        n.attribute("label").or_else(|| n.attribute("id")).unwrap_or("Captions").into(),
                        CaptionFormat::Subtitle,
                    );
                    track.language = n.attribute("language").unwrap_or("und").into();
                    for cue in n.children().filter(|c| c.has_tag_name("cue")) {
                        let start = tick(number(cue, "start", 0.)?)?;
                        let end = tick(number(cue, "end", 0.)?)?;
                        if start < Tick::ZERO || end <= start {
                            log.warn("Caption cue dropped: invalid or empty interval")?;
                            continue;
                        }
                        let text = cue.attribute("text").map(str::to_owned).unwrap_or_else(|| {
                            cue.children().filter(|n| n.has_tag_name("word")).filter_map(|n| n.attribute("text")).collect::<Vec<_>>().join(" ")
                        });
                        if text.len() > MAX_OUTPUT {
                            return Err(err("caption text limit exceeded"));
                        }
                        track.captions.push(Caption {
                            id: ClipId(b.alloc()),
                            start,
                            duration: sub(end, start)?,
                            text,
                            speaker: cue.attribute("speaker").map(str::to_owned),
                            cue_id: None,
                            settings: String::new(),
                        });
                        rest(cue, &["start", "end", "text", "speaker"], log)?;
                    }
                    track.captions.sort_by_key(|c| c.start);
                    // FilmCraft caption tracks cannot overlap. Split overlapping cues without discarding text.
                    let mut lanes: Vec<CaptionTrack> = Vec::new();
                    for cue in std::mem::take(&mut track.captions) {
                        lane_work = lane_work.saturating_add(lanes.len());
                        if lane_work > 2_000_000 {
                            return Err(err("caption overlap mapping work limit exceeded"));
                        }
                        let lane = lanes.iter().position(|l| l.captions.last().is_none_or(|c| c.end() <= cue.start));
                        if let Some(i) = lane {
                            lanes.get_mut(i).ok_or_else(|| err("caption lane missing"))?.captions.push(cue);
                        } else {
                            let mut lane = track.clone();
                            lane.id = TrackId(b.alloc());
                            lane.captions.push(cue);
                            lanes.push(lane);
                        }
                    }
                    if lanes.len() > 1 {
                        log.warn("Overlapping captions split into separate caption tracks")?;
                    }
                    if lanes.is_empty() {
                        lanes.push(track);
                    }
                    seq.caption_tracks.extend(lanes);
                    rest(n, &["id", "label", "language"], log)?;
                }
            }
            "audioMix" => {
                rest(container, &[], log)?;
                for n in container.children().filter(Node::is_element) {
                    match n.tag_name().name() {
                        "audioTrack" => audio(n, doc, r, assets, b, seq, log)?,
                        "master" => {
                            seq.master_volume_db = common::gain_to_db(values::unit(n.attribute("volume").unwrap_or("1")).map_err(err)?);
                            rest(n, &["volume"], log)?;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    seq.markers.sort_by_key(|m| m.start);
    Ok(())
}
fn audio(n: Node<'_, '_>, doc: &SceneDoc<'_>, r: &Resolved, assets: &HashMap<String, Asset>, b: &mut Builder, seq: &mut Sequence, log: &mut Log) -> Result<()> {
    let id = n.attribute("asset").ok_or_else(|| err("audioTrack needs asset"))?;
    let asset = assets.get(id).ok_or_else(|| err("audioTrack asset missing"))?;
    let name = n.attribute("id").unwrap_or("Audio");
    let mut start = tick(number(n, "start", 0.)?)?;
    if let Some(id) = n.attribute("startMarker") {
        let marker = doc.doc.descendants().find(|m| m.has_tag_name("marker") && m.attribute("id") == Some(id));
        if let Some(marker) = marker {
            start = add(start, tick(number(marker, "time", 0.)?)?)?;
        } else {
            log.warn("Unmapped audioTrack generated startMarker; track skipped")?;
            return Ok(());
        }
    }
    let input = tick(number(n, "clipIn", 0.)?)?;
    let speed = number(n, "speed", 1.)?.abs().max(0.001);
    let duration = doc
        .doc
        .descendants()
        .find(|a| a.attribute("id") == Some(id))
        .and_then(|a| a.attribute("duration"))
        .map(|s| values::double(s).map_err(err))
        .transpose()?;
    let output = if n.attribute("clipOut").is_some() {
        tick(number(n, "clipOut", 0.)?)?
    } else if let Some(d) = duration {
        tick(d)?
    } else {
        log.warn("Audio media duration unknown: source span inferred from project end")?;
        add(input, scaled_ticks(sub(tick(r.project.duration)?, start)?.max(Tick::ZERO), speed)?)?
    };
    if input < Tick::ZERO || output < input || start < Tick::ZERO {
        return Err(err("invalid audioTrack interval"));
    }
    let play = playback_ticks(sub(output, input)?, speed)?;
    let project_end = tick(r.project.duration)?;
    let mut track = b.track(TrackKind::Audio, seq.audio_tracks.len());
    track.name = name.into();
    track.volume_db = finite(common::gain_to_db(values::unit(n.attribute("volume").unwrap_or("1")).map_err(err)?) + number(n, "gain", 0.)?)?;
    track.pan = number(n, "pan", 0.)?.clamp(-1., 1.) * 100.;
    track.muted = flag(n, "mute", false)?;
    let plays = values::integer_in(n.attribute("loop").unwrap_or("0"), 0, MAX_CLIPS.saturating_sub(1) as i64).map_err(err)?.saturating_add(1);
    if seq.audio_tracks.iter().map(|t| t.items.len()).sum::<usize>().saturating_add(usize::try_from(plays).map_err(|_| err("audio loop count"))?) > MAX_CLIPS {
        return Err(err("audioMix clip limit exceeded"));
    }
    for i in 0..plays {
        let offset = Tick(play.0.checked_mul(i).ok_or_else(|| err("audio loop offset overflow"))?);
        let begin = add(start, offset)?;
        let end = add(begin, play)?.min(project_end);
        let source = input;
        if end <= begin {
            continue;
        }
        let mut c = b.clip(asset.item, TrackKind::Audio, name, begin, sub(end, begin)?, source);
        c.speed = speed;
        c.reverse = flag(n, "reverse", false)?;
        if c.reverse {
            c.source_in = sub(output, scaled_ticks(c.duration, speed)?)?;
        }
        c.audio_stream = asset.audio_stream;
        add(source, scaled_ticks(c.duration, speed)?)?;
        track.items.push(c);
    }
    seq.audio_tracks.push(track);
    rest(n, &["id", "asset", "start", "startMarker", "clipIn", "clipOut", "speed", "reverse", "volume", "gain", "pan", "mute", "loop"], log)?;
    Ok(())
}

fn playback_ticks(source: Tick, speed: f64) -> Result<Tick> {
    finite(speed)?;
    if source < Tick::ZERO || speed < 0.001 {
        return Err(err("invalid audio playback ratio"));
    }
    // Divide by the exact binary rational, avoiding an f64 reciprocal's rounding error.
    let bits = speed.to_bits();
    let exponent = i32::try_from((bits >> 52) & 0x7ff).map_err(|_| err("audio speed exponent"))? - 1023 - 52;
    let mantissa = i128::from((bits & ((1u64 << 52) - 1)) | (1u64 << 52));
    let (numerator, denominator) = if exponent < 0 {
        let factor = 1i128.checked_shl(exponent.unsigned_abs()).ok_or_else(|| err("audio speed divisor overflow"))?;
        (i128::from(source.0).checked_mul(factor).ok_or_else(|| err("audio duration overflow"))?, mantissa)
    } else {
        let factor = 1i128.checked_shl(exponent.unsigned_abs()).filter(|n| *n > 0);
        let Some(denominator) = factor.and_then(|f| mantissa.checked_mul(f)) else { return Ok(Tick::ZERO) };
        (i128::from(source.0), denominator)
    };
    let q = numerator / denominator;
    let remainder = numerator % denominator;
    let other = denominator - remainder;
    let rounded = q + i128::from(remainder > other || (remainder == other && q % 2 != 0));
    i64::try_from(rounded).map(Tick).map_err(|_| err("audio duration exceeds Tick range"))
}
