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
        let mut curves = BTreeMap::new();
        for (index, &(id, seconds)) in a.keys.iter().enumerate() {
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
            let k = Keyframe::new(tick(seconds)?, ParamValue::Float(value));
            let next = a.keys.get(index.saturating_add(1)).and_then(|(id, _)| doc.doc.get_node(*id));
            curves.insert(k.time, interpolation(key, next, a.default_interpolation.as_str(), log)?);
            mapped.keyframes.push(k);
        }
        mapped.keyframes.sort_by_key(|k| k.time);
        mapped = map_curves(mapped, &curves, r.project.fps, log, sampled_keys)?;
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
        let signature = |k: &Keyframe| (k.time, k.interp, k.in_influence, k.out_influence);
        let synchronized = xp.keyframes.is_empty() || yp.keyframes.is_empty() || xp.keyframes.iter().map(signature).eq(yp.keyframes.iter().map(signature));
        let mut times = BTreeMap::new();
        for k in xp.keyframes.iter().chain(&yp.keyframes) {
            if times.len() >= MAX_KEYS {
                return Err(err("Motion keyframe limit exceeded"));
            }
            times.insert(k.time, k.clone());
        }
        if !synchronized {
            if let (Some((&first, _)), Some((&last, _))) = (times.first_key_value(), times.last_key_value()) {
                let start = r.project.fps.frame_at(first);
                let end = r.project.fps.frame_at(last);
                if i128::from(end) - i128::from(start) > MAX_KEYS as i128 {
                    return Err(err("baked Motion keyframe limit exceeded"));
                }
                for frame in start..=end {
                    let time = r.project.fps.tick_of(frame);
                    if time > first && time < last {
                        if times.len() >= MAX_KEYS {
                            return Err(err("baked Motion keyframe limit exceeded"));
                        }
                        times.entry(time).or_insert_with(|| Keyframe::new(time, ParamValue::Float(0.)));
                    }
                }
            }
            log.warn("Independent Motion axis curves baked at sequence frame rate")?;
        }
        p.keyframes.clear();
        for (time, mut key) in times {
            key.value = ParamValue::Vec2(Vec2::new(finite(xp.f64_at(time))?, finite(yp.f64_at(time))?));
            if !synchronized {
                key.interp = Interpolation::Linear;
            }
            push_sample(&mut p.keyframes, key, sampled_keys)?;
        }
    }
    Ok(Channels { params, mute })
}

#[derive(Clone, Copy)]
enum Curve {
    Linear,
    Hold,
    Css([f64; 4]),
}

// Adapted from scene-render crates/sr-eval/src/curve.rs (resolve), author's grant 2026-10-13.
fn interpolation(key: Node<'_, '_>, next: Option<Node<'_, '_>>, default: &str, log: &mut Log) -> Result<Curve> {
    for attr in ["spatialIn", "spatialOut"] {
        if key.attribute(attr).is_some() {
            log.warn(format!("Unmapped key {attr} handle"))?;
        }
    }
    Ok(match key.attribute("interpolation").unwrap_or(default) {
        "linear" => Curve::Linear,
        "hold" => Curve::Hold,
        "ease-in" => Curve::Css([0.42, 0., 1., 1.]),
        "ease-out" => Curve::Css([0., 0., 0.58, 1.]),
        "ease-in-out" => Curve::Css([0.42, 0., 0.58, 1.]),
        "cubic-bezier" => {
            let points = if let Some(raw) = key.attribute("bezier") {
                let v = raw.split(',').take(5).map(|s| values::double(s.trim()).map_err(err)).collect::<Result<Vec<_>>>()?;
                let [x1, y1, x2, y2] = v.as_slice() else { return Err(err("CSS bezier needs four numbers")) };
                [*x1, *y1, *x2, *y2]
            } else {
                let handle = |raw: Option<&str>| -> Result<(f64, f64)> {
                    let Some(raw) = raw else { return Ok((1. / 3., 1.)) };
                    let v = raw.split(',').take(3).map(|s| values::double(s.trim()).map_err(err)).collect::<Result<Vec<_>>>()?;
                    let [influence, speed] = v.as_slice() else { return Err(err("temporal handle needs two numbers")) };
                    Ok((finite(*influence)?.clamp(0., 1.), finite(*speed)?))
                };
                let (out, out_speed) = handle(key.attribute("easeOut"))?;
                let (input, in_speed) = handle(next.and_then(|n| n.attribute("easeIn")))?;
                [out, finite(out * out_speed)?, 1. - input, finite(1. - input * in_speed)?]
            };
            let [x1, y1, x2, y2] = points;
            for v in points {
                finite(v)?;
            }
            if !(0. ..=1.).contains(&x1) || !(0. ..=1.).contains(&x2) {
                return Err(err("CSS bezier x outside 0..1"));
            }
            Curve::Css([x1, y1, x2, y2])
        }
        name => {
            log.warn(format!("Unmapped interpolation {name}; using linear"))?;
            Curve::Linear
        }
    })
}

fn map_curves(mut p: Param, curves: &BTreeMap<Tick, Curve>, fps: FrameRate, log: &mut Log, sampled_keys: &mut usize) -> Result<Param> {
    if p.keyframes.len() > MAX_KEYS {
        return Err(err("animation keyframe limit exceeded"));
    }
    let mut incoming = vec![None; p.keyframes.len()];
    let mut outgoing = incoming.clone();
    let mut exact = true;
    for (index, pair) in p.keyframes.windows(2).enumerate() {
        let [a, _] = pair else { return Err(err("missing animation segment")) };
        if let Some(Curve::Css([x1, y1, x2, y2])) = curves.get(&a.time).copied() {
            if x1 == y1 && x2 == y2 {
                continue;
            }
            let out = if y1 == 0. && (0.01..=1.).contains(&x1) {
                Some(x1)
            } else if x1 == 1. / 3. && y1 == 1. / 3. {
                None
            } else {
                exact = false;
                None
            };
            let input = if y2 == 1. && (0.01..=1.).contains(&(1. - x2)) {
                Some(1. - x2)
            } else if x2 == 2. / 3. && y2 == 2. / 3. {
                None
            } else {
                exact = false;
                None
            };
            if let Some(slot) = outgoing.get_mut(index) {
                *slot = out;
            }
            if let Some(slot) = incoming.get_mut(index.saturating_add(1)) {
                *slot = input;
            }
        }
    }
    for (index, key) in p.keyframes.iter_mut().enumerate() {
        let input = incoming.get(index).copied().flatten();
        let out = outgoing.get(index).copied().flatten();
        key.interp = if matches!(curves.get(&key.time), Some(Curve::Hold)) {
            exact &= input.is_none();
            Interpolation::Hold
        } else {
            match (input.is_some(), out.is_some()) {
                (true, true) => Interpolation::AutoBezier,
                (true, false) => Interpolation::EaseIn,
                (false, true) => Interpolation::EaseOut,
                (false, false) => Interpolation::Linear,
            }
        };
        key.in_influence = input.unwrap_or(1. / 3.);
        key.out_influence = out.unwrap_or(1. / 3.);
    }
    if exact {
        return Ok(p);
    }
    let mut baked = Vec::new();
    for pair in p.keyframes.windows(2) {
        let [a, b] = pair else { return Err(err("missing animation segment")) };
        let span = sub(b.time, a.time)?;
        let curve = curves.get(&a.time).copied().unwrap_or(Curve::Linear);
        let mut first = a.clone();
        first.interp = if matches!(curve, Curve::Hold) { Interpolation::Hold } else { Interpolation::Linear };
        push_sample(&mut baked, first, sampled_keys)?;
        if span.0 <= 0 || matches!(curve, Curve::Hold) {
            continue;
        }
        let start = fps.frame_at(a.time).checked_add(1).ok_or_else(|| err("animation frame overflow"))?;
        let end = fps.frame_at(b.time);
        if i128::from(end) - i128::from(start) > MAX_KEYS as i128 {
            return Err(err("baked animation keyframe limit exceeded"));
        }
        for frame in start..=end {
            let time = fps.tick_of(frame);
            if time >= b.time {
                break;
            }
            let u = sub(time, a.time)?.0 as f64 / span.0 as f64;
            let progress = match curve {
                Curve::Css(points) => css_progress(points, u)?,
                _ => u,
            };
            let av = a.value.as_f64().ok_or_else(|| err("non-scalar animation"))?;
            let bv = b.value.as_f64().ok_or_else(|| err("non-scalar animation"))?;
            let value = finite(av + finite(bv - av)? * progress)?;
            push_sample(&mut baked, Keyframe::new(time, ParamValue::Float(value)), sampled_keys)?;
        }
    }
    if let Some(last) = p.keyframes.last() {
        push_sample(&mut baked, Keyframe::new(last.time, last.value.clone()), sampled_keys)?;
    }
    p.keyframes = baked;
    log.warn("CSS bezier baked at sequence frame rate; frame values preserved")?;
    Ok(p)
}

fn push_sample(keys: &mut Vec<Keyframe>, key: Keyframe, total: &mut usize) -> Result<()> {
    if *total >= MAX_KEYS {
        return Err(err("baked animation keyframe limit exceeded"));
    }
    *total = total.saturating_add(1);
    keys.push(key);
    Ok(())
}

fn css_progress([x1, y1, x2, y2]: [f64; 4], x: f64) -> Result<f64> {
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..64 {
        let s = (lo + hi) / 2.;
        let v = 1. - s;
        let at = 3. * v * v * s * x1 + 3. * v * s * s * x2 + s * s * s;
        if at < x {
            lo = s;
        } else {
            hi = s;
        }
    }
    let s = (lo + hi) / 2.;
    let v = 1. - s;
    finite(3. * v * v * s * y1 + 3. * v * s * s * y2 + s * s * s)
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
            let mut reverse = Vec::new();
            for (index, original) in p.keyframes.iter().enumerate().rev() {
                let previous = index.checked_sub(1).and_then(|i| p.keyframes.get(i));
                let next = p.keyframes.get(index.saturating_add(1));
                let mut key = original.clone();
                let out = previous.is_some_and(|a| {
                    a.interp != Interpolation::Hold
                        && (matches!(
                            original.interp,
                            Interpolation::AutoBezier | Interpolation::Bezier | Interpolation::ContinuousBezier | Interpolation::EaseIn
                        ) || matches!(a.interp, Interpolation::Bezier | Interpolation::ContinuousBezier))
                });
                let input = next.is_some()
                    && matches!(original.interp, Interpolation::AutoBezier | Interpolation::Bezier | Interpolation::ContinuousBezier | Interpolation::EaseOut);
                key.interp = match (input, out) {
                    (true, true) => Interpolation::AutoBezier,
                    (true, false) => Interpolation::EaseIn,
                    (false, true) => Interpolation::EaseOut,
                    (false, false) => Interpolation::Linear,
                };
                std::mem::swap(&mut key.in_influence, &mut key.out_influence);
                if let Some(a) = previous.filter(|a| a.interp == Interpolation::Hold && original.time < a.time) {
                    key.interp = Interpolation::Hold;
                    reverse.push(key);
                    let mut held = a.clone();
                    held.time = add(original.time, Tick(1))?;
                    if held.time < a.time {
                        reverse.push(held);
                    }
                } else {
                    reverse.push(key);
                }
                if reverse.len() > MAX_KEYS {
                    return Err(err("reversed keyframe limit exceeded"));
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn opacity(curve: &str) -> (Param, Report) {
        let xml = format!(
            r#"<scene version="1.7"><project width="100" height="100" fps="24" duration="1"/><assets><image id="i" src="i.png" width="100" height="100"/></assets><composition><layer id="a" asset="i" end="1"><animate property="opacity"><key time="0" value="0" {curve}/><key time="1" value="1"/></animate></layer></composition></scene>"#
        );
        let (p, _, report) = super::super::import(xml.as_bytes(), &ImportOptions::default()).unwrap();
        let c = p.project.sequence(p.sequences[0]).unwrap().video_tracks.iter().flat_map(|t| &t.items).find(|c| c.name == "a").unwrap();
        (c.effect("opacity").unwrap().param("opacity").unwrap().clone(), report)
    }

    #[test]
    fn scene_import_named_ease_quarters() {
        let (p, _) = opacity(r#"interpolation="ease-in-out""#);
        // CSS (0.42,0,0.58,1): solve x(s)=1/4,1/2,3/4; y=3s²-2s³.
        for (quarter, value) in [(1, 12.9161931047), (2, 50.), (3, 87.0838068953)] {
            assert!((p.f64_at(Tick(filmcraft_time::TICKS_PER_SECOND * quarter / 4)) - value).abs() < 1e-6);
        }
        assert_eq!(p.keyframes.len(), 2);
    }

    #[test]
    fn scene_import_css_baked_frame_values() {
        let (p, report) = opacity(r#"interpolation="cubic-bezier" bezier="0.3333333333333333,0.2,0.6666666666666666,0.8""#);
        // x(s)=s; y(s)=.6s+1.2s²-.8s³. This slope cannot use flat FilmCraft handles.
        for frame in 0..=24 {
            let u = f64::from(frame) / 24.;
            let value = 100. * (0.6 * u + 1.2 * u * u - 0.8 * u * u * u);
            assert!((p.f64_at(Tick(filmcraft_time::TICKS_PER_SECOND * i64::from(frame) / 24)) - value).abs() < 1e-6);
        }
        assert!(report.mentions("baked"));
    }
}

#[cfg(test)]
mod additional_tests {
    use super::*;

    #[test]
    fn scene_import_named_in_out_quarters() {
        // CSS ease-in (0.42,0,1,1), solving x(s)=u, y(s)=3s²-2s³.
        // ease-out is its reflection about (1/2,1/2).
        for (name, expected) in [("ease-in", [9.3464650004, 31.5356812573, 62.1861869174]), ("ease-out", [37.8138130826, 68.4643187427, 90.6535349996])] {
            let (p, report) = tests::opacity(&format!(r#"interpolation="{name}""#));
            for (quarter, value) in (1..=3).zip(expected) {
                let actual = p.f64_at(Tick(filmcraft_time::TICKS_PER_SECOND * quarter / 4));
                assert!((actual - value).abs() < 1e-6, "{name} {quarter}: {actual} != {value}");
            }
            assert!(report.mentions("baked"));
        }
    }

    #[test]
    fn scene_import_motion_css_handles() {
        for animate_y in [false, true] {
            let y = if animate_y { r#"<animate property="y"><key time="0" value="0"/><key time="1" value="100"/></animate>"# } else { "" };
            let xml = format!(
                r#"<scene version="1.7"><project width="100" height="100" fps="24" duration="1"/><assets><image id="i" src="i.png" width="100" height="100"/></assets><composition><layer id="a" asset="i" end="1" y="0"><animate property="x"><key time="0" value="0" interpolation="ease-in-out"/><key time="1" value="100"/></animate>{y}</layer></composition></scene>"#
            );
            let (p, _, _) = super::super::import(xml.as_bytes(), &ImportOptions::default()).unwrap();
            let c = p.project.sequence(p.sequences[0]).unwrap().video_tracks.iter().flat_map(|t| &t.items).find(|c| c.name == "a").unwrap();
            let at = c.effect("motion").unwrap().param("position").unwrap().vec2_at(Tick(filmcraft_time::TICKS_PER_SECOND / 4));
            // CSS ease-in-out x at 1/4; the independent y is linear or static.
            assert!((at.x - 12.9161931047).abs() < 1e-6, "{at:?}");
            assert!((at.y - if animate_y { 25. } else { 0. }).abs() < 1e-6);
        }
    }

    #[test]
    fn scene_import_baking_is_bounded_and_nonfinite_rejected() {
        let base = r#"<scene version="1.7"><project width="100" height="100" fps="24" duration="10000"/><assets><image id="i" src="i.png" width="100" height="100"/></assets><composition><layer id="a" asset="i" end="10000"><animate property="opacity"><key time="0" value="0" interpolation="ease-in"/><key time="10000" value="1"/></animate></layer></composition></scene>"#;
        for xml in [base.to_string(), base.replace("ease-in", "cubic-bezier\" bezier=\"0,NaN,1,1")] {
            let result = std::panic::catch_unwind(|| super::super::import(xml.as_bytes(), &ImportOptions::default()));
            assert!(result.is_ok());
            assert!(result.unwrap().is_err());
        }
    }
}
