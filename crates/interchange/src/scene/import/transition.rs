use super::*;
use filmcraft_project::{Param, ParamValue, Transition, TransitionAlign, TransitionId};

/// Whether one overlay retains both the transition definition and all its endpoints.
pub(super) fn preserved_in_overlay(r: &Resolved, tr: &resolve::Transition, covered: &HashSet<usize>) -> bool {
    let Some(endpoint) = tr.from.or(tr.to) else { return false };
    if !tr.from.into_iter().chain(tr.to).all(|i| covered.contains(&i)) {
        return false;
    }
    match tr.node {
        Some(i) => covered.contains(&i),
        // Auto-transitions are defined by the enclosing sequence's attributes.
        None => r.node(endpoint).and_then(|n| n.parent).and_then(|i| r.node(i)).is_some_and(|n| n.class == NodeClass::Sequence && covered.contains(&n.index)),
    }
}

pub(super) fn apply(
    doc: &SceneDoc<'_>,
    r: &Resolved,
    b: &mut Builder,
    seq: &mut Sequence,
    log: &mut Log,
    snaps: &mut Snaps,
    clock: Option<(Tick, f64)>,
    preserved_transitions: &HashSet<usize>,
) -> Result<()> {
    let root = clock.is_none();
    let (origin, rate) = clock.unwrap_or((Tick::ZERO, 1.));
    let nested_names: HashSet<_> = if root {
        b.p.items.values().filter_map(|i| i.as_sequence()).flat_map(|s| s.video_tracks.iter()).flat_map(|t| t.items.iter()).map(|c| c.name.clone()).collect()
    } else {
        HashSet::new()
    };
    let mut clips: HashMap<&str, Vec<(usize, &TrackItem)>> = HashMap::new();
    for (index, track) in seq.video_tracks.iter().enumerate() {
        for c in &track.items {
            clips.entry(&c.name).or_default().push((index, c));
        }
    }
    let mut endpoints = HashMap::new();
    for (name, mut clips) in clips {
        clips.sort_by_key(|(_, c)| c.start);
        if let (Some((first_track, first)), Some((last_track, last))) = (clips.first(), clips.last()) {
            endpoints.insert(name.to_owned(), ((*first_track, first.id), (*last_track, last.id)));
        }
    }
    for (i, tr) in r.transitions.iter().enumerate() {
        if preserved_transitions.contains(&i) {
            continue;
        }
        let locate = |index: Option<usize>, from: bool| {
            let name = index.and_then(|i| r.node(i)).map(|n| n.id.as_str())?;
            endpoints.get(name).map(|(first, last)| if from { *last } else { *first })
        };
        let from = locate(tr.from, true);
        let to = locate(tr.to, false);
        if from.is_none() && to.is_none() {
            if root && !tr.from.into_iter().chain(tr.to).filter_map(|i| r.node(i)).all(|n| nested_names.contains(&n.id)) {
                log.warn("Unmapped transition: no native endpoint clip was retained")?;
            }
            continue;
        }
        if tr.from.is_some() != from.is_some() || tr.to.is_some() != to.is_some() {
            log.warn("Unmapped transition: endpoint is absent or in another sequence")?;
            continue;
        }
        if from.zip(to).is_some_and(|(a, b)| a.0 != b.0) {
            log.warn("Unmapped transition: clips landed on different tracks; both clips retained")?;
            continue;
        }
        let Some(track) = from.or(to).map(|p| p.0) else { continue };
        let effect = match tr.kind.as_str() {
            "crossfade" => "cross_dissolve",
            "additive-dissolve" => "additive_dissolve",
            "dip-to-color" => "dip_to_color",
            _ => {
                log.warn(format!("Unmapped transition type {}", tr.kind))?;
                continue;
            }
        };
        let mut effect = filmcraft_project::find_effect(effect).ok_or_else(|| err("transition definition missing"))?.instance();
        if tr.kind == "dip-to-color" {
            effect.params.insert("color".into(), Param::new(ParamValue::Color([0., 0., 0., 1.])));
        }
        let xml = tr.node.and_then(|i| r.node(i)).map(|n| xml_node(doc, n)).transpose()?;
        if let Some(xml) = xml {
            for a in xml.attributes() {
                if !matches!(a.name(), "id" | "from" | "to" | "type" | "duration" | "alignment" | "curve" | "color") {
                    log.warn(format!("Unmapped transition@{}", a.name()))?;
                }
            }
            if let Some(color) = xml.attribute("color")
                && let Paint::Color(c) = values::color(color).map_err(err)?
            {
                effect.params.insert("color".into(), Param::new(ParamValue::Color(c.0)));
            }
        }
        if tr.curve != "linear" {
            log.warn(format!("Unmapped transition curve {}; linear progress retained", tr.curve))?;
        }
        let start = snaps.edge(scaled_ticks(sub(tick(tr.window.start)?, origin)?, rate)?, r.project.fps)?;
        let end = snaps.edge(scaled_ticks(sub(tick(tr.window.end)?, origin)?, rate)?, r.project.fps)?;
        if end <= start {
            log.warn("Transition dropped: zero frames")?;
            continue;
        }
        let align = match xml.and_then(|x| x.attribute("alignment")).unwrap_or("center") {
            "start" => TransitionAlign::StartAtCut,
            "end" => TransitionAlign::EndAtCut,
            _ => TransitionAlign::CenterAtCut,
        };
        let target = seq.video_tracks.get_mut(track).ok_or_else(|| err("transition track missing"))?;
        target.transitions.push(Transition {
            id: TransitionId(b.alloc()),
            effect,
            start,
            duration: sub(end, start)?,
            from: from.map(|p| p.1),
            to: to.map(|p| p.1),
            align,
            reverse: false,
        });
    }
    Ok(())
}

pub(super) fn occupancy(r: &Resolved, n: &ResolvedNode) -> Result<(Tick, Tick)> {
    let mut interval = n.span;
    // Inherited handles remain inside a nest's source. Only this node's own handles
    // are removed from its carrier, whose edges are the nominal edit points.
    for tr in &r.transitions {
        if tr.from == Some(n.index) || tr.to == Some(n.index) {
            interval = n.interval;
            break;
        }
    }
    for tr in &r.transitions {
        if tr.from.is_some() && tr.to.is_some() {
            if tr.from == Some(n.index) {
                interval.end = tr.cut;
            }
            if tr.to == Some(n.index) {
                interval.start = tr.cut;
            }
        }
    }
    Ok((tick(interval.start)?, tick(interval.end)?))
}

pub(super) fn place(r: &Resolved, b: &mut Builder, seq: &mut Sequence, clips: Vec<TrackItem>, log: &mut Log) -> Result<()> {
    let mut endpoints: HashMap<&str, (usize, usize)> = HashMap::new();
    for (i, c) in clips.iter().enumerate() {
        endpoints
            .entry(&c.name)
            .and_modify(|(first, last)| {
                if clips.get(*first).is_some_and(|other| c.start < other.start) {
                    *first = i;
                }
                if clips.get(*last).is_some_and(|other| c.end() > other.end()) {
                    *last = i;
                }
            })
            .or_insert((i, i));
    }
    let mut pairs = Vec::new();
    for tr in &r.transitions {
        let a = tr.from.and_then(|i| r.node(i)).and_then(|n| endpoints.get(n.id.as_str())).map(|p| p.1);
        let z = tr.to.and_then(|i| r.node(i)).and_then(|n| endpoints.get(n.id.as_str())).map(|p| p.0);
        let Some((a, z)) = a.zip(z) else { continue };
        if clips.get(a).zip(clips.get(z)).is_some_and(|(a, z)| a.end() == z.start) {
            pairs.push((a, z));
        }
    }
    if pairs.is_empty() {
        return super::place(b, seq, TrackKind::Video, clips);
    }
    let spans: Vec<_> = clips.iter().map(|c| stack::Span { start: c.start, end: c.end() }).collect();
    let mut tracks: Vec<_> = stack::assign(&spans).into_iter().map(|t| usize::try_from(t).map_err(|_| err("track index overflow"))).collect::<Result<_>>()?;
    let mut work = 0usize;
    let mut constraints = Vec::new();
    let mut by_start: Vec<_> = clips.iter().enumerate().collect();
    by_start.sort_by_key(|(_, c)| c.start);
    for (pos, &(i, a)) in by_start.iter().enumerate() {
        for &(j, z) in by_start.iter().skip(pos.saturating_add(1)).take_while(|(_, c)| c.start < a.end()) {
            budget(&mut work, 1)?;
            if z.end() > a.start {
                constraints.push((i.min(j), i.max(j)));
            }
        }
    }
    let mut components: Vec<_> = (0..clips.len()).collect();
    for (a, z) in pairs {
        budget(&mut work, clips.len())?;
        let mut candidate = components.clone();
        let ca = *candidate.get(a).ok_or_else(|| err("transition component missing"))?;
        let cz = *candidate.get(z).ok_or_else(|| err("transition component missing"))?;
        if ca == cz {
            continue;
        }
        for component in &mut candidate {
            if *component == cz {
                *component = ca;
            }
        }
        // Reject only this pair if its equality contradicts paint order. Other
        // independent dissolves still get their own joint-placement attempt.
        if let Some(assigned) = joint_tracks(&candidate, &constraints, &mut work)? {
            tracks = assigned;
            components = candidate;
        } else {
            log.warn("Transition pair cannot share a track without changing paint order; nominal clips retained")?;
        }
    }
    for (c, track) in clips.into_iter().zip(tracks) {
        b.ensure_tracks(seq, TrackKind::Video, track.saturating_add(1));
        seq.video_tracks.get_mut(track).ok_or_else(|| err("joint track missing"))?.items.push(c);
    }
    for track in &mut seq.video_tracks {
        track.sort();
    }
    Ok(())
}
fn budget(work: &mut usize, amount: usize) -> Result<()> {
    *work = work.saturating_add(amount);
    if *work > 2_000_000 {
        return Err(err("joint transition placement work limit exceeded"));
    }
    Ok(())
}
fn joint_tracks(components: &[usize], constraints: &[(usize, usize)], work: &mut usize) -> Result<Option<Vec<usize>>> {
    budget(work, components.len().saturating_add(constraints.len()))?;
    let mut edges: Vec<HashSet<usize>> = vec![HashSet::new(); components.len()];
    let mut degree = vec![0usize; components.len()];
    for &(a, z) in constraints {
        let from = *components.get(a).ok_or_else(|| err("paint component missing"))?;
        let to = *components.get(z).ok_or_else(|| err("paint component missing"))?;
        if from == to {
            return Ok(None);
        }
        if edges.get_mut(from).ok_or_else(|| err("paint edge missing"))?.insert(to) {
            let d = degree.get_mut(to).ok_or_else(|| err("paint degree missing"))?;
            *d = d.saturating_add(1);
        }
    }
    let mut ready: Vec<_> = degree.iter().enumerate().filter(|(_, d)| **d == 0).map(|(i, _)| i).collect();
    let mut levels = vec![0usize; components.len()];
    let mut visited = 0usize;
    while let Some(i) = ready.pop() {
        visited = visited.saturating_add(1);
        let level = levels.get(i).copied().ok_or_else(|| err("paint track missing"))?.saturating_add(1);
        for &to in edges.get(i).ok_or_else(|| err("paint edge missing"))? {
            let track = levels.get_mut(to).ok_or_else(|| err("paint track missing"))?;
            *track = (*track).max(level);
            let d = degree.get_mut(to).ok_or_else(|| err("paint degree missing"))?;
            *d = d.saturating_sub(1);
            if *d == 0 {
                ready.push(to);
            }
        }
    }
    if visited != components.len() {
        return Ok(None);
    }
    components.iter().map(|&c| levels.get(c).copied().ok_or_else(|| err("joint track missing"))).collect::<Result<Vec<_>>>().map(Some)
}
