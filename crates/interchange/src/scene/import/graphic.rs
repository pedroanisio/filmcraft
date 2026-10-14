use super::*;
use filmcraft_geom::Vec2;
use filmcraft_project::{EffectInstance, ItemKind, Label, Param, ParamValue, graphic};

fn set(e: &mut EffectInstance, name: &str, v: ParamValue) {
    e.params.insert(name.into(), Param::new(v));
}
fn number(n: Node<'_, '_>, name: &str, default: f64) -> Result<f64> {
    n.attribute(name).map_or(Ok(default), |s| values::double(s).map_err(err))
}
fn color(n: Node<'_, '_>, name: &str, default: &str, log: &mut Log) -> Result<[f32; 4]> {
    match values::paint(n.attribute(name).unwrap_or(default)).map_err(err)? {
        Paint::Color(c) => Ok(c.0),
        _ => {
            log.warn(format!("Unmapped {}@{name} paint; transparent fill retained", n.tag_name().name()))?;
            Ok([0.; 4])
        }
    }
}
fn point(x: f64, y: f64) -> Result<[f32; 2]> {
    let (x, y) = (finite(x)?, finite(y)?);
    if x.abs() > f64::from(f32::MAX) || y.abs() > f64::from(f32::MAX) {
        return Err(err("graphic point exceeds f32 range"));
    }
    Ok([x as f32, y as f32])
}

pub(super) fn clip(doc: &SceneDoc<'_>, r: &Resolved, n: &ResolvedNode, b: &mut Builder, log: &mut Log, snaps: &mut Snaps) -> Result<Option<TrackItem>> {
    let xml = xml_node(doc, n)?;
    let l = n.layout.as_ref().ok_or_else(|| err("graphic has no resolved layout"))?;
    let size = Vec2::new(l.width, l.height);
    let mut centered = false;
    let mut e = if n.class == NodeClass::Text {
        let asset =
            doc.doc.descendants().find(|a| a.has_tag_name("text") && a.attribute("id") == xml.attribute("asset")).ok_or_else(|| err("text asset missing"))?;
        let text = asset.attribute("text").map(str::to_owned).unwrap_or_else(|| {
            asset.children().filter(|n| n.has_tag_name("span")).flat_map(|n| n.children().filter(Node::is_text)).filter_map(|n| n.text()).collect()
        });
        let font_size = number(asset, "size", 48.)?;
        let mut e = graphic::new_text_layer(&text, Vec2::ZERO, font_size);
        let families = asset.attribute("font").unwrap_or("Inter");
        let family = families.split(',').map(|s| s.trim().trim_matches(['\'', '"'])).find(|s| !s.is_empty()).unwrap_or("Inter");
        if families.contains(',') {
            log.warn("Unmapped font fallback chain; first family retained for FilmCraft font resolution")?;
        }
        set(&mut e, "font", ParamValue::Text(family.into()));
        let weight = values::integer_in(asset.attribute("weight").unwrap_or("400"), 1, 1000).map_err(err)?;
        let style = match weight {
            100 => "Thin",
            200 => "ExtraLight",
            300 => "Light",
            400 => "Regular",
            500 => "Medium",
            600 => "SemiBold",
            700 => "Bold",
            800 => "ExtraBold",
            900 => "Black",
            _ => {
                log.warn("Unmapped numeric font weight; nearest regular/bold face retained")?;
                if weight >= 600 { "Bold" } else { "Regular" }
            }
        };
        let italic = asset.attribute("fontStyle").is_some_and(|s| s != "normal");
        set(&mut e, "font_style", ParamValue::Text(if italic { format!("{style} Italic") } else { style.into() }));
        set(
            &mut e,
            "align",
            ParamValue::Choice(match asset.attribute("align") {
                Some("center") => 1,
                Some("end") => 2,
                Some("justify") => 3,
                _ => 0,
            }),
        );
        set(&mut e, "box_width", ParamValue::Float(size.x));
        set(&mut e, "box_height", ParamValue::Float(size.y));
        set(&mut e, "fill_color", ParamValue::Color(color(asset, "color", "#FFFFFFFF", log)?));
        let stroke = color(asset, "strokeColor", "#000000FF", log)?;
        let sw = number(
            asset,
            "strokeWidth",
            if asset.attribute("strokeColor").is_some() || asset.attribute("strokePosition").is_some() { finite(font_size * 0.06)? } else { 0. },
        )?;
        set(&mut e, "stroke", ParamValue::Bool(sw > 0.));
        set(&mut e, "stroke_color", ParamValue::Color(stroke));
        set(&mut e, "stroke_width", ParamValue::Float(sw));
        set(
            &mut e,
            "stroke_type",
            ParamValue::Choice(match asset.attribute("strokePosition") {
                Some("inside") => 2,
                Some("outside") => 0,
                _ => 1,
            }),
        );
        for a in asset.attributes() {
            if !matches!(
                a.name(),
                "id" | "text"
                    | "font"
                    | "fontStyle"
                    | "weight"
                    | "size"
                    | "width"
                    | "height"
                    | "align"
                    | "color"
                    | "strokeColor"
                    | "strokeWidth"
                    | "strokePosition"
            ) {
                log.warn(format!("Unmapped text@{}", a.name()))?;
            }
        }
        if asset.children().any(|n| n.has_tag_name("span")) {
            log.warn("Unmapped text span styles; concatenated text retained")?;
        }
        e
    } else {
        let shape = xml.attribute("shape").unwrap_or("rect");
        let (kind, points) = match shape {
            "rect" | "rounded-rect" => (0, Vec::new()),
            "ellipse" => (1, Vec::new()),
            "polygon" if xml.attribute("outerRadius").is_none() => (2, Vec::new()),
            "polygon" | "star" => (3, radial(xml, size, shape == "star")?),
            "path" => {
                let Some(p) = path(xml.attribute("path").unwrap_or(""))? else {
                    log.warn("Unmapped SVG path: only a single closed M/L/H/V polygon is representable")?;
                    return Ok(None);
                };
                (3, p)
            }
            _ => {
                log.warn(format!("Unmapped shape {shape}"))?;
                return Ok(None);
            }
        };
        centered = kind != 3;
        let mut e = graphic::new_shape_layer(kind, Vec2::ZERO, size, points);
        let sides = values::integer_in(xml.attribute("points").unwrap_or("5"), 1, 4096).map_err(err)?.max(3);
        set(&mut e, "sides", ParamValue::Float(sides.min(64) as f64));
        if kind == 2 && sides > 64 {
            log.warn("Polygon sides limited to graphic model maximum of 64")?;
        }
        set(&mut e, "corner_radius", ParamValue::Float(number(xml, "radius", 0.)?));
        set(&mut e, "fill_color", ParamValue::Color(color(xml, "fill", "#FFFFFFFF", log)?));
        let sw = number(xml, "strokeWidth", 0.)?;
        set(&mut e, "stroke", ParamValue::Bool(sw > 0.));
        set(&mut e, "stroke_width", ParamValue::Float(sw));
        set(&mut e, "stroke_color", ParamValue::Color(color(xml, "stroke", "#00000000", log)?));
        set(&mut e, "stroke_type", ParamValue::Choice(1));
        for a in xml.attributes() {
            if !matches!(a.name(), "shape" | "path" | "points" | "radius" | "fill" | "stroke" | "strokeWidth" | "innerRadius" | "outerRadius")
                && !super::mapped_node_attribute(a.name())
            {
                log.warn(format!("Unmapped shape@{}", a.name()))?;
            }
        }
        e
    };
    let (start, end) = transition::occupancy(r, n)?;
    let start = snaps.edge(start, r.project.fps)?;
    let end = snaps.edge(end, r.project.fps)?;
    if end <= start {
        log.warn("Video clip dropped: zero frames")?;
        return Ok(None);
    }
    let item = b.p.add_item(&n.id, Label::Lavender, ItemKind::Graphic { width: r.project.width, height: r.project.height, rate: r.project.fps }, None);
    let mut c = b.clip(item, TrackKind::Video, &n.id, start, sub(end, start)?, Tick::ZERO);
    let mut samples = 0;
    let channels = properties::channels(doc, r, n, log, &mut samples)?;
    let mut keys = 0;
    properties::apply(&mut c, TrackKind::Video, n, &channels, &mut keys, log)?;
    for effect in &c.effects {
        if matches!(effect.effect.as_str(), "motion" | "opacity") {
            for (name, p) in &effect.params {
                if e.params.contains_key(name) {
                    let mut p = p.clone();
                    if name == "anchor" && centered {
                        offset_anchor(&mut p, size)?;
                    }
                    e.params.insert(name.clone(), p);
                }
            }
        }
    }
    c.effects = filmcraft_project::effect::intrinsic_video();
    set(&mut e, "name", ParamValue::Text(xml.attribute("name").unwrap_or(&n.id).into()));
    c.effects.push(e);
    c.enabled = n.visible;
    group::relative_opacity(&mut c, r, n)?;
    Ok(Some(c))
}
fn offset_anchor(p: &mut Param, size: Vec2) -> Result<()> {
    let adjust = |v: &mut ParamValue| -> Result<()> {
        if let ParamValue::Vec2(v) = v {
            *v = Vec2::new(finite(v.x - size.x / 2.)?, finite(v.y - size.y / 2.)?);
        }
        Ok(())
    };
    adjust(&mut p.value)?;
    for k in &mut p.keyframes {
        adjust(&mut k.value)?;
    }
    Ok(())
}
fn radial(n: Node<'_, '_>, size: Vec2, star: bool) -> Result<Vec<[f32; 2]>> {
    let tips = values::integer_in(n.attribute("points").unwrap_or("5"), 1, 4096).map_err(err)?.max(if star { 2 } else { 3 });
    let count = if star { tips * 2 } else { tips };
    let mut out = Vec::new();
    let outer = n.attribute("outerRadius").map(|s| values::double(s).map_err(err)).transpose()?;
    let inner = n.attribute("innerRadius").map(|s| values::double(s).map_err(err)).transpose()?;
    for i in 0..count {
        let inside = star && i % 2 == 1;
        let factor = if inside { 0.5 } else { 1. };
        let rx = if inside { inner.or(outer.map(|v| v * 0.5)) } else { outer }.unwrap_or(size.x / 2. * factor);
        let ry = if inside { inner.or(outer.map(|v| v * 0.5)) } else { outer }.unwrap_or(size.y / 2. * factor);
        let angle = std::f64::consts::TAU * (i as f64) / (count as f64) - std::f64::consts::FRAC_PI_2;
        out.push(point(size.x / 2. + rx * angle.cos(), size.y / 2. + ry * angle.sin())?);
    }
    Ok(out)
}

// The graphic model stores one closed polygon, so curves and multiple subpaths must be reported.
fn path(raw: &str) -> Result<Option<Vec<[f32; 2]>>> {
    if raw.len() > 1 << 20 {
        return Err(err("graphic path byte limit exceeded"));
    }
    let mut tokens = Vec::new();
    let mut begin = None;
    let mut prev = ' ';
    for (i, c) in raw.char_indices() {
        let command = c.is_ascii_alphabetic() && c != 'e' && c != 'E';
        let separate = c.is_whitespace() || c == ',' || command || ((c == '-' || c == '+') && prev != 'e' && prev != 'E');
        if separate && let Some(b) = begin.take() {
            tokens.push(raw.get(b..i).ok_or_else(|| err("path token boundary"))?);
        }
        if command {
            tokens.push(raw.get(i..i + c.len_utf8()).ok_or_else(|| err("path command boundary"))?);
        } else if !c.is_whitespace() && c != ',' && begin.is_none() {
            begin = Some(i);
        }
        prev = c;
        if tokens.len() > 32768 {
            return Err(err("graphic path token limit exceeded"));
        }
    }
    if let Some(b) = begin {
        tokens.push(raw.get(b..).ok_or_else(|| err("path token boundary"))?);
    }
    let mut out = Vec::new();
    let (mut x, mut y) = (0., 0.);
    let mut cmd = "";
    let mut i = 0;
    let mut closed = false;
    while let Some(token) = tokens.get(i) {
        if token.len() == 1 && token.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            cmd = token;
            i += 1;
            if matches!(cmd, "Z" | "z") {
                closed = true;
                break;
            }
            if !matches!(cmd, "M" | "m" | "L" | "l" | "H" | "h" | "V" | "v") {
                return Ok(None);
            }
            if matches!(cmd, "M" | "m") && !out.is_empty() {
                return Ok(None);
            }
        }
        let Some(a) = tokens.get(i) else { return Ok(None) };
        let a = values::double(a).map_err(err)?;
        i += 1;
        match cmd {
            "H" => x = a,
            "h" => x = finite(x + a)?,
            "V" => y = a,
            "v" => y = finite(y + a)?,
            "M" | "m" | "L" | "l" => {
                let Some(b) = tokens.get(i) else { return Ok(None) };
                let b = values::double(b).map_err(err)?;
                i += 1;
                if matches!(cmd, "m" | "l") {
                    x = finite(x + a)?;
                    y = finite(y + b)?;
                } else {
                    x = a;
                    y = b;
                }
                if cmd == "M" {
                    cmd = "L"
                } else if cmd == "m" {
                    cmd = "l"
                }
            }
            _ => return Ok(None),
        }
        out.push(point(x, y)?);
        if out.len() > 8192 {
            return Err(err("graphic path point limit exceeded"));
        }
    }
    Ok(if closed && i == tokens.len() && out.len() >= 3 { Some(out) } else { None })
}
