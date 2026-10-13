use super::*;
use filmcraft_geom::Vec2;
use filmcraft_project::{Interpolation, Keyframe, Param, ParamValue};

pub(super) struct Channels {
    params: Vec<(&'static str, &'static str, Param)>,
    mute: bool,
}

fn scalar(v: f64) -> Result<Param> {
    Ok(Param::new(ParamValue::Float(finite(v)?)))
}
fn pair(x: f64, y: f64) -> Result<Param> {
    Ok(Param::new(ParamValue::Vec2(Vec2::new(finite(x)?, finite(y)?))))
}

pub(super) fn channels(doc: &SceneDoc<'_>, r: &Resolved, n: &ResolvedNode, log: &mut Log, sampled_keys: &mut usize) -> Result<Channels> {
    let xml = xml_node(doc, n)?;
    let mut params = Vec::new();
    if let Some(l) = &n.layout {
        let t = l.transform;
        let (mut x, mut y, mut sx, mut sy, mut rot) = (t.x, t.y, t.scale_x, t.scale_y, t.rotation);
        if n.transform_parent.is_some()
            && let Some(m) = l.world_matrix
        {
            sx = finite(m.a.hypot(m.b))?;
            if sx > 0. {
                sy = finite((m.a * m.d - m.b * m.c) / sx)?;
                rot = finite(m.b.atan2(m.a).to_degrees())?;
                if finite(m.a * m.c + m.b * m.d)?.abs() > 1e-8 {
                    log.warn("Unmapped skew from composed parent transform")?;
                }
            } else {
                sy = finite(m.c.hypot(m.d))?;
                rot = finite((-m.c).atan2(m.d).to_degrees())?;
            }
            x = finite(m.e + m.a * t.anchor_x + m.c * t.anchor_y)?;
            y = finite(m.f + m.b * t.anchor_x + m.d * t.anchor_y)?;
        }
        params.extend([
            ("motion", "position", pair(x, y)?),
            ("motion", "anchor", pair(t.anchor_x, t.anchor_y)?),
            ("motion", "scale_width", scalar(sx * 100.)?),
            ("motion", "scale", scalar(sy * 100.)?),
            ("motion", "rotation", scalar(rot)?),
            ("motion", "uniform_scale", Param::new(ParamValue::Bool(false))),
        ]);
        if xml.attribute("fit").is_some_and(|s| s != "none") {
            log.warn("Unmapped fit resampling: resolver provides the fit box but no fitted content matrix")?;
        }
    }
    params.push(("opacity", "opacity", scalar(n.opacity * 100.)?));
    let volume = values::unit(xml.attribute("volume").unwrap_or("1")).map_err(err)?;
    let mute = flag(xml, "mute", false)?;
    params.push(("volume", "level", scalar(common::gain_to_db(volume))?));
    let parent_opacity = n.parent.and_then(|i| r.node(i)).map_or(1., |p| p.opacity);
    let parent_size = n
        .parent
        .and_then(|i| r.node(i))
        .and_then(|p| p.layout.as_ref())
        .map_or((f64::from(r.project.width), f64::from(r.project.height)), |l| (l.width, l.height));
    let mut axes: HashMap<&'static str, Param> = HashMap::new();
    for a in &n.animations {
        if a.target != n.xml {
            log.warn(format!("Unmapped animate property {} on nested element", a.property))?;
            continue;
        }
        let (effect, param, axis, factor) = match a.property.as_str() {
            "opacity" => ("opacity", "opacity", None, 100. * parent_opacity),
            "rotation" => ("motion", "rotation", None, 1.),
            "scaleX" => ("motion", "scale_width", None, 100.),
            "scaleY" => ("motion", "scale", None, 100.),
            "x" => ("motion", "position", Some("x"), 1.),
            "y" => ("motion", "position", Some("y"), 1.),
            "anchorX" => ("motion", "anchor", Some("anchorX"), 1.),
            "anchorY" => ("motion", "anchor", Some("anchorY"), 1.),
            "volume" => ("volume", "level", None, 1.),
            _ => {
                log.warn(format!("Unmapped animate property {}", a.property))?;
                continue;
            }
        };
        let mut mapped = scalar(0.)?;
        for &(id, seconds) in &a.keys {
            let key = doc.doc.get_node(id).ok_or_else(|| err("animation key is missing"))?;
            let raw = key.attribute("value").ok_or_else(|| err("animation key needs value"))?;
            let value = if let Some(axis) = axis {
                let parent = if matches!(axis, "x" | "anchorX") { parent_size.0 } else { parent_size.1 };
                finite(values::length(raw).map_err(err)?.resolve(parent, f64::from(r.project.width), f64::from(r.project.height)))?
            } else if a.property == "volume" {
                values::unit(raw).map_err(err)?
            } else {
                finite(values::double(raw).map_err(err)? * factor)?
            };
            let mut k = Keyframe::new(tick(seconds)?, ParamValue::Float(value));
            k.interp = interpolation(key, a.default_interpolation.as_str(), log)?;
            mapped.keyframes.push(k);
        }
        mapped.keyframes.sort_by_key(|k| k.time);
        if a.property == "volume" {
            mapped = volume_curve(mapped, log, sampled_keys)?;
        }
        if let Some(first) = mapped.keyframes.first() {
            mapped.value = first.value.clone();
        }
        if let Some(axis) = axis {
            axes.insert(axis, mapped);
        } else if let Some((_, _, p)) = params.iter_mut().find(|(e, p, _)| *e == effect && *p == param) {
            *p = mapped;
        }
    }
    for (param, x, y) in [("position", "x", "y"), ("anchor", "anchorX", "anchorY")] {
        if !axes.contains_key(x) && !axes.contains_key(y) {
            continue;
        }
        let Some((_, _, p)) = params.iter_mut().find(|(e, p, _)| *e == "motion" && *p == param) else {
            continue;
        };
        let base = p.value.as_vec2().ok_or_else(|| err("Motion point is missing"))?;
        let xp = axes.remove(x).unwrap_or(scalar(base.x)?);
        let yp = axes.remove(y).unwrap_or(scalar(base.y)?);
        let mut times: BTreeMap<Tick, Interpolation> = BTreeMap::new();
        for k in xp.keyframes.iter().chain(&yp.keyframes) {
            times.insert(k.time, k.interp);
        }
        if !xp.keyframes.is_empty()
            && !yp.keyframes.is_empty()
            && xp.keyframes.iter().map(|k| (k.time, k.interp)).collect::<Vec<_>>() != yp.keyframes.iter().map(|k| (k.time, k.interp)).collect::<Vec<_>>()
        {
            log.warn("Independent Motion axis curves approximated on their combined key times")?;
        }
        p.keyframes = times
            .into_iter()
            .map(|(time, interp)| -> Result<Keyframe> {
                let value = ParamValue::Vec2(Vec2::new(finite(xp.f64_at(time))?, finite(yp.f64_at(time))?));
                let mut key = Keyframe::new(time, value);
                key.interp = interp;
                Ok(key)
            })
            .collect::<Result<_>>()?;
    }
    Ok(Channels { params, mute })
}

fn interpolation(key: Node<'_, '_>, default: &str, log: &mut Log) -> Result<Interpolation> {
    let name = key.attribute("interpolation").unwrap_or(default);
    for attr in ["easeIn", "easeOut", "spatialIn", "spatialOut"] {
        if key.attribute(attr).is_some() {
            log.warn(format!("Unmapped key {attr} handle"))?;
        }
    }
    Ok(match name {
        "linear" => Interpolation::Linear,
        "hold" => Interpolation::Hold,
        "ease-in" => Interpolation::EaseIn,
        "ease-out" => Interpolation::EaseOut,
        "ease-in-out" => {
            log.warn("ease-in-out approximated by Auto Bezier")?;
            Interpolation::AutoBezier
        }
        "cubic-bezier" => {
            log.warn("CSS bezier approximated by closest FilmCraft interpolation; temporal and spatial handles are not CSS control points")?;
            if let Some(raw) = key.attribute("bezier") {
                let v = raw.split(',').take(5).map(|s| values::double(s.trim()).map_err(err)).collect::<Result<Vec<_>>>()?;
                let [x1, y1, x2, y2] = v.as_slice() else {
                    return Err(err("CSS bezier needs four numbers"));
                };
                if *x1 < 0. || *x1 > 1. || *x2 < 0. || *x2 > 1. {
                    return Err(err("CSS bezier x outside 0..1"));
                }
                let candidates = [
                    ([0., 0., 1., 1.], Interpolation::Linear),
                    ([0.42, 0., 1., 1.], Interpolation::EaseIn),
                    ([0., 0., 0.58, 1.], Interpolation::EaseOut),
                    ([0.42, 0., 0.58, 1.], Interpolation::AutoBezier),
                ];
                let mut best = (f64::MAX, Interpolation::Linear);
                for (c, interp) in candidates {
                    let distance = c.into_iter().zip([*x1, *y1, *x2, *y2]).try_fold(0., |sum, (a, b)| finite(sum + finite((a - b) * (a - b))?))?;
                    if distance < best.0 {
                        best = (distance, interp);
                    }
                }
                best.1
            } else {
                Interpolation::Bezier
            }
        }
        _ => {
            log.warn(format!("Unmapped interpolation {name}; using linear"))?;
            Interpolation::Linear
        }
    })
}

fn volume_curve(amplitude: Param, log: &mut Log, sampled_keys: &mut usize) -> Result<Param> {
    let mut db = scalar(common::gain_to_db(amplitude.value.as_f64().unwrap_or(0.)))?;
    if let (Some(first), Some(last)) = (amplitude.keyframes.first(), amplitude.keyframes.last()) {
        sub(last.time, first.time)?;
    }
    let mut sampled = false;
    for pair in amplitude.keyframes.windows(2) {
        let [a, b] = pair else { return Err(err("missing volume key pair")) };
        let span = sub(b.time, a.time)?;
        // At default handle influence, supported eases have slope <= 2. Sampling at
        // 4096 intervals keep amplitude error below .001, including integer-tick rounding.
        let steps = if a.interp == Interpolation::Hold { 1 } else { span.0.clamp(1, 4096) };
        sampled |= steps > 1;
        for i in 0..steps {
            let offset = i128::from(span.0) * i128::from(i) / i128::from(steps);
            let time = add(a.time, Tick(i64::try_from(offset).map_err(|_| err("volume sampling overflow"))?))?;
            if db.keyframes.last().is_some_and(|k: &Keyframe| k.time == time) {
                continue;
            }
            if *sampled_keys >= MAX_KEYS {
                return Err(err("sampled volume keyframe limit exceeded"));
            }
            let mut key = Keyframe::new(time, ParamValue::Float(common::gain_to_db(finite(amplitude.f64_at(time))?)));
            key.interp = if steps == 1 { Interpolation::Hold } else { Interpolation::Linear };
            *sampled_keys = sampled_keys.saturating_add(1);
            db.keyframes.push(key);
        }
    }
    if let Some(last) = amplitude.keyframes.last() {
        if *sampled_keys >= MAX_KEYS {
            return Err(err("sampled volume keyframe limit exceeded"));
        }
        *sampled_keys = sampled_keys.saturating_add(1);
        db.keyframes.push(Keyframe::new(last.time, ParamValue::Float(common::gain_to_db(last.value.as_f64().unwrap_or(0.)))));
    }
    if sampled {
        log.warn("Animated volume approximated with sampled dB keys: absolute amplitude error < 0.001; at most 4096 intervals per key segment")?;
    }
    Ok(db)
}

fn effect_time(c: &TrackItem, composition: Tick) -> Result<Tick> {
    let offset = scaled_ticks(sub(composition, c.start)?, c.speed.abs())?;
    if c.reverse { sub(sub(add(c.source_in, scaled_ticks(c.duration, c.speed.abs())?)?, offset)?, Tick(1)) } else { add(c.source_in, offset) }
}

pub(super) fn apply(c: &mut TrackItem, kind: TrackKind, _n: &ResolvedNode, channels: &Channels, keys: &mut usize, _log: &mut Log) -> Result<()> {
    for (effect, name, param) in &channels.params {
        if (kind == TrackKind::Audio) != (*effect == "volume") {
            continue;
        }
        let mut p = param.clone();
        for key in &mut p.keyframes {
            key.time = effect_time(c, key.time)?;
        }
        if c.reverse {
            // Reversing key order also reverses the outgoing segment's interpolation.
            let mut reverse = Vec::new();
            for pair in p.keyframes.windows(2).rev() {
                let [a, b] = pair else { return Err(err("missing reverse key pair")) };
                let mut key = b.clone();
                key.interp = match a.interp {
                    Interpolation::EaseIn => Interpolation::EaseOut,
                    Interpolation::EaseOut => Interpolation::EaseIn,
                    other => other,
                };
                std::mem::swap(&mut key.in_influence, &mut key.out_influence);
                reverse.push(key);
                if a.interp == Interpolation::Hold && b.time < a.time {
                    let mut held = a.clone();
                    held.time = add(b.time, Tick(1))?;
                    if held.time < a.time {
                        reverse.push(held);
                    }
                }
                if reverse.len() >= MAX_KEYS {
                    return Err(err("reversed keyframe limit exceeded"));
                }
            }
            if let Some(first) = p.keyframes.first() {
                reverse.push(first.clone());
            }
            p.keyframes = reverse;
        }
        *keys = keys.saturating_add(p.keyframes.len());
        if *keys > MAX_KEYS {
            return Err(err("expanded import keyframe limit exceeded"));
        }
        if let (Some(first), Some(last)) = (p.keyframes.first(), p.keyframes.last()) {
            sub(last.time, first.time)?;
        }
        common::set_param(c, effect, name, p);
    }
    if kind == TrackKind::Audio {
        c.gain_db = if channels.mute { -287.5 } else { 0. };
    }
    Ok(())
}
