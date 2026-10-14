use super::*;
use filmcraft_project::{Param, ParamValue};

pub(super) fn nested(n: &ResolvedNode) -> bool {
    matches!(n.class, NodeClass::Group { isolated: true, .. } | NodeClass::Group { retimed: true, .. })
}
pub(super) fn fold(
    doc: &SceneDoc<'_>,
    r: &Resolved,
    b: &mut Builder,
    video: &mut Vec<TrackItem>,
    audio: &mut Vec<TrackItem>,
    log: &mut Log,
    snaps: &mut Snaps,
) -> Result<()> {
    // A group that already has a clip was preserved as an overlay, including its descendants.
    let represented: HashSet<_> = video.iter().map(|c| c.name.as_str()).collect();
    let mut groups: Vec<_> = r
        .nodes
        .iter()
        .filter(|n| nested(n))
        .filter(|n| {
            let mut current = Some(*n);
            for _ in 0..=128 {
                let Some(node) = current else { break };
                if represented.contains(node.id.as_str()) {
                    return false;
                }
                current = node.parent.and_then(|i| r.node(i));
            }
            true
        })
        .collect();
    if groups.len().saturating_mul(video.len().saturating_add(audio.len()).saturating_add(groups.len()).saturating_add(r.transitions.len())) > 2_000_000 {
        return Err(err("nested sequence mapping work limit exceeded"));
    }
    groups.sort_by_key(|n| std::cmp::Reverse(n.depth));
    let owners: HashMap<_, _> = r
        .nodes
        .iter()
        .map(|n| {
            let mut parent = n.parent;
            let mut owner = None;
            for _ in 0..128 {
                let Some(p) = parent.and_then(|i| r.node(i)) else { break };
                if nested(p) {
                    owner = Some(p.index);
                    break;
                }
                parent = p.parent;
            }
            (n.id.as_str(), owner)
        })
        .collect();
    let order: HashMap<_, _> = r.nodes.iter().map(|n| (n.id.as_str(), n.paint_position.saturating_add(1))).collect();
    for n in groups {
        let xml = xml_node(doc, n)?;
        let speed = values::double(xml.attribute("timeScale").unwrap_or("1")).map_err(err)?;
        if n.container_clock.scale <= 0. || speed <= 0. {
            return Err(err("invalid group clock scale"));
        }
        let rate = finite(speed / n.container_clock.scale)?;
        if rate <= 0. {
            return Err(err("group clock underflow"));
        }
        let (audio_start, audio_end) = transition::occupancy(r, n)?;
        let start = snaps.edge(audio_start, r.project.fps)?;
        let end = snaps.edge(audio_end, r.project.fps)?;
        // Source zero precedes both carriers, including pre-roll handles. Audio never snaps.
        let origin = tick(n.span.start)?.min(start);
        let source_end = tick(n.span.end)?.max(end);
        let settings = common::settings_for(r.project.fps, r.project.width, r.project.height, false);
        let mut seq = common::empty_sequence(settings.clone());
        for (kind, clips) in [(TrackKind::Video, &mut *video), (TrackKind::Audio, &mut *audio)] {
            let mut remain = Vec::new();
            let mut children = Vec::new();
            for mut c in clips.drain(..) {
                if owners.get(c.name.as_str()).copied().flatten() != Some(n.index) {
                    remain.push(c);
                    continue;
                }
                let end = scaled_ticks(sub(c.end(), origin)?, rate)?;
                c.start = scaled_ticks(sub(c.start, origin)?, rate)?;
                c.duration = sub(end, c.start)?;
                c.speed = finite(c.speed / rate)?;
                if c.speed <= 0. {
                    return Err(err("nested clip speed underflow"));
                }
                children.push(c);
            }
            *clips = remain;
            children.sort_by_key(|c| order.get(c.name.as_str()).copied().unwrap_or(0));
            if kind == TrackKind::Video {
                transition::place(r, b, &mut seq, children, log)?;
            } else {
                place(b, &mut seq, kind, children)?;
            }
        }
        let has_audio = !seq.audio_tracks.is_empty() && audio_end > audio_start;
        if end <= start && !has_audio {
            log.warn("Nested group dropped: zero frames")?;
            continue;
        }
        transition::apply(doc, r, b, &mut seq, log, snaps, Some((origin, rate)))?;
        let source_duration = scaled_ticks(sub(source_end, origin)?, rate)?;
        seq.mark_out = Some(source_duration);
        retain_duration(b, &mut seq, source_duration)?;
        let id = b.reserve_sequence(&n.id, settings, None);
        b.put_sequence(id, seq);
        let link = has_audio.then(|| b.link_id());
        if end > start {
            let source_in = scaled_ticks(sub(start, origin)?, rate)?;
            let mut c = b.clip(id, TrackKind::Video, &n.id, start, sub(end, start)?, source_in);
            c.speed = rate;
            c.enabled = n.visible;
            c.link = link;
            if matches!(n.class, NodeClass::Group { isolated: true, .. }) {
                common::set_param(&mut c, "opacity", "opacity", Param::new(ParamValue::Float(finite(n.opacity * 100.)?)));
                relative_opacity(&mut c, r, n)?;
            }
            video.push(c);
        } else {
            log.warn("Nested video carrier dropped: zero frames; exact audio retained")?;
        }
        if has_audio {
            let source_in = scaled_ticks(sub(audio_start, origin)?, rate)?;
            let mut c = b.clip(id, TrackKind::Audio, &n.id, audio_start, sub(audio_end, audio_start)?, source_in);
            c.speed = rate;
            c.link = link;
            c.enabled = n.visible;
            audio.push(c);
        }
        if n.animations.iter().any(|a| a.property == "opacity") {
            log.warn("Unmapped animated isolated group opacity; static opacity retained")?;
        }
        if flag(xml, "clip", false)? {
            log.warn("Group clip box is not represented; full-frame nested canvas retained")?;
        }
    }
    video.sort_by_key(|c| order.get(c.name.as_str()).copied().unwrap_or(0));
    audio.sort_by_key(|c| order.get(c.name.as_str()).copied().unwrap_or(0));
    Ok(())
}
fn retain_duration(b: &mut Builder, seq: &mut Sequence, duration: Tick) -> Result<()> {
    let start = seq.duration();
    if start >= duration {
        return Ok(());
    }
    let s = &seq.settings;
    let item = b.generator_media(
        &format!("@group-duration-{}", b.p.items.len()),
        "Scene Duration",
        Generator::TransparentVideo,
        &MediaSpec { duration: Some(duration), video: Some((s.width, s.height, s.frame_rate)), ..Default::default() },
        None,
    );
    let mut c = b.clip(item, TrackKind::Video, "Scene Duration", start, sub(duration, start)?, Tick::ZERO);
    c.enabled = false;
    b.ensure_tracks(seq, TrackKind::Video, 1);
    seq.video_tracks.first_mut().ok_or_else(|| err("missing group duration track"))?.items.push(c);
    Ok(())
}

pub(super) fn relative_opacity(c: &mut TrackItem, r: &Resolved, n: &ResolvedNode) -> Result<()> {
    let mut parent = n.parent;
    for _ in 0..128 {
        let Some(p) = parent.and_then(|i| r.node(i)) else { break };
        if matches!(p.class, NodeClass::Group { isolated: true, .. }) {
            // Zero opacity is already invisible; division cannot recover it and is unnecessary.
            if p.opacity <= 0. {
                return Ok(());
            }
            let effect = if c.graphic_layers().next().is_some() {
                c.effects.iter_mut().find(|e| filmcraft_project::graphic::is_layer(e))
            } else {
                c.effect_mut("opacity")
            };
            if let Some(param) = effect.and_then(|e| e.params.get_mut("opacity")) {
                for value in std::iter::once(&mut param.value).chain(param.keyframes.iter_mut().map(|k| &mut k.value)) {
                    if let ParamValue::Float(v) = value {
                        *v = finite(*v / p.opacity)?;
                    }
                }
            }
            break;
        }
        parent = p.parent;
    }
    Ok(())
}

pub(super) fn overlay_roots(r: &Resolved, roots: &mut HashSet<usize>, log: &mut Log) -> Result<()> {
    // Keep in sync with render::MAX_NEST_DEPTH without an L2 -> L3 dependency.
    const MAX_NATIVE_DEPTH: usize = 8;
    for n in &r.nodes {
        if !nested(n) {
            continue;
        }
        let mut count = 1usize;
        let mut parent = n.parent;
        for _ in 0..128 {
            let Some(p) = parent.and_then(|i| r.node(i)) else { break };
            if nested(p) {
                count = count.saturating_add(1);
            }
            parent = p.parent;
        }
        if count > MAX_NATIVE_DEPTH {
            roots.insert(n.index);
            log.warn("Native nesting depth exceeded: subtree preserved as scene overlay")?;
        }
    }
    let original: Vec<_> = roots.iter().copied().collect();
    for index in original {
        let mut target = index;
        let mut parent = r.node(index).and_then(|n| n.parent);
        for _ in 0..128 {
            let Some(p) = parent.and_then(|i| r.node(i)) else { break };
            if matches!(p.class, NodeClass::Group { isolated: true, .. }) {
                target = p.index;
            }
            parent = p.parent;
        }
        if target != index {
            roots.remove(&index);
            roots.insert(target);
            log.warn("Isolated subtree retained as one overlay to preserve composite opacity")?;
        }
    }
    Ok(())
}
