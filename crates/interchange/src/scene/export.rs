//! Export sequences as sr-core XML, using the smallest version that holds the emitted constructs.
//!
//! Emitted version gates (scene-render.sch; XSD introduction notes permit new attributes on 1.0
//! elements): markers and captions → 1.1 (V1); transitions → 1.1 (V3). All other emitted constructs
//! (media/text assets, layers, shapes, groups, animate/key, audioMix/audioTrack) are available in 1.0.
//! Matching linked AV clips use the video's own audio. Independent audio uses audioMix tracks
//! with audio-only assets, so a picture layer cannot create an extra audio placement.

#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Write};

use filmcraft_media::MediaKind;
use filmcraft_project::{
    ClipId, EffectInstance, Interpolation, ItemId, ItemKind, Keyframe, MarkerKind, MediaRef, Param, ParamValue, Project, Sequence, Track, TrackItem,
    TransitionAlign, graphic,
};
use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

use super::{Limits, read, validate, values};
use crate::{Error, ExportOptions, Report, Result};

const MAX_NODES: usize = 100_000;
// Default effects expand each imported clip into many project parameters, not XML nodes.
const MAX_WORK: usize = 4_000_000;
const MAX_XML: usize = 32 << 20;
const MAX_VALUE: usize = 65_536;
const MAX_DEPTH: usize = 32;
const MAX_TIME: i64 = TICKS_PER_SECOND * 1_000_000;

fn failure(message: &str) -> Error {
    Error::Other(format!("scene export: {message}"))
}

struct BoundedValue(String);
impl Write for BoundedValue {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if self.0.len().saturating_add(text.len()) > MAX_VALUE {
            return Err(std::fmt::Error);
        }
        self.0.push_str(text);
        Ok(())
    }
}

#[derive(Default)]
struct Xml {
    text: String,
    repaired: usize,
    inexact_times: usize,
    max_time_error: u64,
}

impl Xml {
    fn push(&mut self, text: &str) -> Result<()> {
        if self.text.len().saturating_add(text.len()) > MAX_XML {
            return Err(failure("XML exceeds the 32 MiB export budget"));
        }
        self.text.push_str(text);
        Ok(())
    }

    fn attr(&mut self, name: &str, value: impl Display) -> Result<()> {
        let mut value_text = BoundedValue(String::new());
        write!(&mut value_text, "{value}").map_err(|_| failure("attribute exceeds the 64 KiB export budget"))?;
        let value = value_text.0;
        self.push(" ")?;
        self.push(name)?;
        self.push("=\"")?;
        for c in value.chars() {
            match c {
                '&' => self.push("&amp;")?,
                '<' => self.push("&lt;")?,
                '>' => self.push("&gt;")?,
                '"' => self.push("&quot;")?,
                '\'' => self.push("&apos;")?,
                '\n' => self.push("&#10;")?,
                '\r' => self.push("&#13;")?,
                '\t' => self.push("&#9;")?,
                '\u{0}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}' => {
                    self.push("�")?;
                    self.repaired = self.repaired.saturating_add(1);
                }
                _ => {
                    let mut bytes = [0; 4];
                    self.push(c.encode_utf8(&mut bytes))?;
                }
            }
        }
        self.push("\"")
    }

    fn time(&mut self, name: &str, value: Secs) -> Result<()> {
        self.attr(name, &value.text)?;
        if value.error != 0 {
            self.inexact_times = self.inexact_times.saturating_add(1);
            self.max_time_error = self.max_time_error.max(value.error.unsigned_abs());
        }
        Ok(())
    }

    fn append(&mut self, other: Xml) -> Result<()> {
        self.inexact_times = self.inexact_times.saturating_add(other.inexact_times);
        self.max_time_error = self.max_time_error.max(other.max_time_error);
        self.repaired = self.repaired.saturating_add(other.repaired);
        self.push(&other.text)
    }
}

struct Secs {
    text: String,
    error: i64,
}

/// Preserve exact ticks when possible; otherwise choose the nearest parsed tick, ties downward.
fn seconds(t: Tick) -> Result<Secs> {
    if t.0.unsigned_abs() > Tick::MAX.0 as u64 {
        return Err(failure("time is outside the scene reader's Tick range"));
    }
    let s = t.0 as f64 / TICKS_PER_SECOND as f64;
    let mut nearest: Option<(f64, i64)> = None;
    for candidate in [s, s.next_down(), s.next_up()] {
        let Some(parsed) = values::seconds_to_tick(candidate) else {
            continue;
        };
        let Some(error) = parsed.0.checked_sub(t.0) else {
            continue;
        };
        if error == 0 {
            return Ok(Secs { text: candidate.to_string(), error });
        }
        if nearest.is_none_or(|(previous, old_error)| {
            error.unsigned_abs() < old_error.unsigned_abs() || (error.unsigned_abs() == old_error.unsigned_abs() && candidate < previous)
        }) {
            nearest = Some((candidate, error));
        }
    }
    let (candidate, error) = nearest.ok_or_else(|| failure("time is outside the scene reader's Tick range"))?;
    Ok(Secs { text: candidate.to_string(), error })
}

fn add(a: Tick, b: Tick) -> Option<Tick> {
    a.0.checked_add(b.0).map(Tick)
}

// The speed's binary rational is applied to ticks in integers, rounding only the final tick.
fn ratio(speed: f64) -> Option<(i128, i128)> {
    if !speed.is_finite() || speed <= 0.0 || speed > 1_000_000.0 {
        return None;
    }
    let bits = speed.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023 - 52;
    let mut numerator = i128::from((bits & ((1_u64 << 52) - 1)) | (1_u64 << 52));
    let mut denominator = 1_i128;
    if exponent >= 0 {
        numerator = numerator.checked_mul(1_i128.checked_shl(exponent as u32)?)?;
    } else {
        let shift = exponent.unsigned_abs();
        if shift > 126 {
            return None;
        }
        denominator = denominator.checked_shl(shift)?;
    }
    let common = numerator.trailing_zeros().min(denominator.trailing_zeros());
    Some((numerator >> common, denominator >> common))
}

fn scale(t: Tick, n: i128, d: i128) -> Option<Tick> {
    if d <= 0 || n < 0 {
        return None;
    }
    let value = i128::from(t.0).checked_mul(n)?;
    let q = value.checked_div(d)?;
    let r = value.checked_rem(d)?.abs();
    let twice = r.checked_mul(2)?;
    let q = if twice > d || (twice == d && q % 2 != 0) { q.checked_add(value.signum())? } else { q };
    i64::try_from(q).ok().filter(|v| v.unsigned_abs() <= Tick::MAX.0 as u64).map(Tick)
}

// Inverse of TrackItem::effect_time_at, including its exclusive reverse source end.
fn animation_time(clip: &TrackItem, media: Tick, floor: bool) -> Result<Tick> {
    let (n, d) = ratio(clip.speed.abs()).ok_or_else(|| failure("animation needs a finite nonzero clip speed"))?;
    let origin = if clip.reverse {
        add(clip.source_in, scale(clip.duration, n, d).ok_or_else(|| failure("animation source span overflow"))?)
            .and_then(|t| t.0.checked_sub(1).map(Tick))
            .ok_or_else(|| failure("animation source end overflow"))?
    } else {
        clip.source_in
    };
    let delta = if clip.reverse { origin.0.checked_sub(media.0) } else { media.0.checked_sub(origin.0) };
    let time = if floor {
        delta.and_then(|t| i128::from(t).checked_mul(d)).and_then(|t| t.checked_div_euclid(n)).and_then(|t| i64::try_from(t).ok()).map(Tick)
    } else {
        delta.and_then(|t| scale(Tick(t), d, n))
    };
    time.ok_or_else(|| failure("animation time overflow"))
}

fn window(start: Tick, duration: Tick) -> Option<Tick> {
    let end = add(start, duration)?;
    if start.0 < 0 || duration.0 <= 0 || end.0 > MAX_TIME {
        return None;
    }
    Some(end)
}

fn rate(rate: FrameRate) -> Result<String> {
    let text = if rate.den == 1 { rate.num.to_string() } else { format!("{}/{}", rate.num, rate.den) };
    values::fps(&text).map_err(|_| failure("invalid frame rate"))?;
    Ok(text)
}

fn safe_audio_frame_boundary(time: Tick, rate: FrameRate) -> bool {
    if time.0 < 0 || rate.num <= 0 || rate.den <= 0 {
        return false;
    }
    let frame_ticks = i128::from(TICKS_PER_SECOND) * i128::from(rate.den);
    let Some(scaled) = i128::from(time.0).checked_mul(i128::from(rate.num)) else {
        return false;
    };
    if scaled % frame_ticks != 0 {
        return false;
    }
    let Some(serialized) = seconds(time).ok().and_then(|s| s.text.parse::<f64>().ok()).filter(|s| s.is_finite()) else {
        return false;
    };
    let fps = rate.num as f64 / rate.den as f64;
    if !fps.is_finite() || fps <= 0.0 {
        return false;
    }
    let frame = scaled / frame_ticks;
    let current = frame as f64 / fps;
    let previous = (frame - 1) as f64 / fps;
    // sr-deliver evaluates k/fps; sr-eval uses [start,end). Decimal Tick alignment
    // alone can exclude the first frame or include the exclusive end at NTSC rates.
    current.is_finite() && previous.is_finite() && previous < serialized && serialized <= current
}

pub(crate) fn sniff(bytes: &[u8]) -> bool {
    if !read::sniff(bytes) {
        return false;
    }
    let head = bytes.get(..bytes.len().min(4096)).unwrap_or(&[]);
    let head = String::from_utf8_lossy(head);
    let mut text = head.as_ref();
    text = text.trim_start_matches('\u{feff}');
    loop {
        text = text.trim_start();
        let end = if text.starts_with("<!--") {
            "-->"
        } else if text.starts_with("<?") {
            "?>"
        } else {
            break;
        };
        let Some((_, rest)) = text.split_once(end) else {
            return false;
        };
        text = rest;
    }
    let mut quote = None;
    for (i, c) in text.char_indices() {
        if Some(c) == quote {
            quote = None;
        } else if quote.is_none() && matches!(c, '\'' | '"') {
            quote = Some(c);
        } else if quote.is_none() && c == '>' {
            let Some(tag) = text.get(..i) else {
                return false;
            };
            let xml = format!("{}/>", tag.trim_end().trim_end_matches('/'));
            return roxmltree::Document::parse(&xml).is_ok_and(|d| {
                let root = d.root_element();
                root.has_tag_name("scene") && root.tag_name().namespace().is_none() && root.attribute("version").is_some()
            });
        }
    }
    false
}

#[derive(Clone, Copy)]
struct AudioClock {
    low: Tick,
    high: Tick,
    root: Tick,
    n: i128,
    d: i128,
    gain: f64,
    muted: bool,
}

fn combine_ratio(n: i128, d: i128, other_n: i128, other_d: i128) -> Option<(i128, i128)> {
    fn gcd(mut a: i128, mut b: i128) -> i128 {
        while b != 0 {
            let r = a % b;
            a = b;
            b = r;
        }
        a
    }
    if n <= 0 || d <= 0 || other_n <= 0 || other_d <= 0 {
        return None;
    }
    let a = gcd(n, other_d);
    let b = gcd(other_n, d);
    Some(((n / a).checked_mul(other_n / b)?, (d / b).checked_mul(other_d / a)?))
}

struct Exporter<'a> {
    project: &'a Project,
    opts: &'a ExportOptions,
    report: &'a mut Report,
    assets: Xml,
    asset_ids: BTreeMap<(ItemId, AssetUse), String>,
    layer_audio: BTreeMap<ClipId, LayerAudio>,
    embedded_audio: BTreeSet<ClipId>,
    building: Vec<ItemId>,
    next: usize,
    remaining: usize,
    remaining_work: usize,
    minor: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AssetUse {
    Picture,
    Linked,
    Audio,
}

#[derive(Clone, Copy)]
struct LayerAudio {
    clip: ClipId,
    volume: f64,
    muted: bool,
}

impl Exporter<'_> {
    fn interval(&mut self, start: Tick, duration: Tick) -> Option<Tick> {
        let Some(end) = window(start, duration) else {
            self.report.warn("invalid or zero-length clip interval skipped");
            return None;
        };
        let written = seconds(start).ok().and_then(|s| start.0.checked_add(s.error)).zip(seconds(end).ok().and_then(|s| end.0.checked_add(s.error)));
        if written.is_none_or(|(start, end)| end <= start) {
            self.report.warn("clip interval collapsed at nearest representable times: skipped");
            return None;
        }
        Some(end)
    }

    fn spend(&mut self, count: usize) -> Result<()> {
        self.remaining = self.remaining.checked_sub(count).ok_or_else(|| failure("expanded content exceeds the 100,000 item export budget"))?;
        Ok(())
    }

    fn spend_work(&mut self, count: usize) -> Result<()> {
        self.remaining_work = self.remaining_work.checked_sub(count).ok_or_else(|| failure("project traversal exceeds the 4,000,000 item export budget"))?;
        Ok(())
    }

    fn id(&mut self) -> String {
        self.next = self.next.saturating_add(1);
        format!("fc{}", self.next)
    }

    fn number(&mut self, value: f64, fallback: f64) -> f64 {
        if value.is_finite() {
            value
        } else {
            self.report.warn("non-finite parameter replaced with its default");
            fallback
        }
    }

    fn scalar(&mut self, effect: Option<&EffectInstance>, name: &str, fallback: f64) -> f64 {
        let value = effect.and_then(|e| e.params.get(name)).and_then(|p| p.value.as_f64()).unwrap_or(fallback);
        self.number(value, fallback)
    }

    fn point(&mut self, effect: Option<&EffectInstance>, name: &str, fallback: (f64, f64)) -> (f64, f64) {
        let Some(v) = effect.and_then(|e| e.params.get(name)).and_then(|p| p.value.as_vec2()) else {
            return fallback;
        };
        let x = if v.x.is_nan() { fallback.0 } else { self.number(v.x, fallback.0) };
        let y = if v.y.is_nan() { fallback.1 } else { self.number(v.y, fallback.1) };
        (x, y)
    }

    fn base(&mut self, mut id: ItemId) -> Option<ItemId> {
        for _ in 0..MAX_DEPTH {
            match self.project.item(id).map(|i| &i.kind) {
                Some(ItemKind::Subclip { parent, .. }) => id = *parent,
                Some(_) => return Some(id),
                None => {
                    self.report.warn("missing source item skipped");
                    return None;
                }
            }
        }
        self.report.warn("subclip reference cycle or depth limit: clip skipped");
        None
    }

    fn asset(&mut self, id: ItemId, usage: AssetUse) -> Result<Option<String>> {
        if let Some(asset) = self.asset_ids.get(&(id, usage)) {
            return Ok(Some(asset.clone()));
        }
        let Some(media) = self.project.item(id).and_then(|i| i.as_media()) else {
            return Ok(None);
        };
        let MediaRef::File { path } = &media.media else {
            self.report.warn("synthetic media without an equivalent scene asset skipped");
            return Ok(None);
        };
        if path.len() > MAX_VALUE || self.opts.relative_to.as_ref().is_some_and(|s| s.len() > MAX_VALUE) {
            return Err(failure("media path exceeds the export budget"));
        }
        if media.info.kind == MediaKind::ImageSequence {
            self.report.warn("image sequence media without an explicit filename pattern skipped");
            return Ok(None);
        }
        let video = media.info.video.as_ref().filter(|_| usage != AssetUse::Audio);
        let still = media.info.kind == MediaKind::Still && usage != AssetUse::Audio;
        if let Some(v) = video {
            if filmcraft_project::validate_frame_size(v.width, v.height).is_err() || rate(v.frame_rate).is_err() {
                self.report.warn("media with invalid dimensions or frame rate skipped");
                return Ok(None);
            }
            if !still && (media.info.duration.0 <= 0 || seconds(media.info.duration).is_err()) {
                self.report.warn("video with unknown or unrepresentable duration skipped");
                return Ok(None);
            }
        } else if !media.info.has_audio() {
            self.report.warn("media without video or audio skipped");
            return Ok(None);
        }
        self.spend(1)?;
        let asset = self.id();
        let path = self.opts.relative_to.as_ref().and_then(|base| crate::common::relative_path(path, base)).unwrap_or_else(|| path.clone());
        let mut out = Xml::default();
        out.push(if still {
            "<image"
        } else if video.is_some() {
            "<video"
        } else {
            "<audio"
        })?;
        out.attr("id", &asset)?;
        out.attr("src", path)?;
        if let Some(v) = video {
            out.attr("width", v.width)?;
            out.attr("height", v.height)?;
            if !still {
                out.attr("fps", rate(v.frame_rate)?)?;
                out.time("duration", seconds(media.info.duration)?)?;
                out.attr("hasAudio", usage == AssetUse::Linked && media.info.has_audio())?;
            }
        } else if let Some(audio) = media.info.audio() {
            if audio.sample_rate > 0 {
                out.attr("sampleRate", audio.sample_rate)?;
            }
            if audio.channels > 0 {
                out.attr("channels", audio.channels)?;
            }
            if media.info.duration.0 > 0
                && let Ok(s) = seconds(media.info.duration)
            {
                out.time("duration", s)?;
            }
        }
        out.push("/>\n")?;
        self.assets.append(out)?;
        self.asset_ids.insert((id, usage), asset.clone());
        if media.interpret != Default::default() {
            self.report.warn("media interpretation overrides dropped");
        }
        Ok(Some(asset))
    }

    fn unsupported(&mut self, clip: &TrackItem) -> Result<()> {
        self.spend(clip.markers.len())?;
        self.spend_work(clip.effects.len())?;
        for e in &clip.effects {
            self.spend_work(e.params.len().saturating_add(e.masks.len()))?;
            for _ in &e.masks {
                self.report.warn("effect mask dropped");
            }
            if !e.enabled {
                continue;
            }
            if !matches!(e.effect.as_str(), "motion" | "opacity" | "volume" | "channel_volume" | "panner" | "time_remap") && !graphic::is_layer(e) {
                self.report.warn("effect without a scene equivalent dropped");
            }
            if matches!(e.effect.as_str(), "channel_volume" | "panner" | "time_remap") && modified(e) {
                self.report.warn("channel controls or time-remapping effect dropped");
            }
        }
        for _ in &clip.markers {
            self.report.warn("clip marker dropped; only sequence markers are exported");
        }
        if clip.multicam.is_some() {
            self.report.warn("multicam selection dropped");
        }
        if clip.essential.is_some() {
            self.report.warn("Essential Sound processing dropped");
        }
        if clip.graphic.is_some() {
            self.report.warn("graphic metadata and responsive design dropped");
        }
        if clip.field_options.is_some() || clip.hold_filters {
            self.report.warn("field processing or hold-filters option dropped");
        }
        if clip.audio_stream != 0 || !clip.source_channels.is_empty() {
            self.report.warn("audio stream/channel selection dropped");
        }
        Ok(())
    }

    fn motion(&mut self, out: &mut Xml, clip: &TrackItem, frame: (u32, u32), source: (u32, u32)) -> Result<()> {
        let effect = clip.effect("motion").filter(|e| e.enabled);
        let position = self.point(effect, "position", (f64::from(frame.0) / 2.0, f64::from(frame.1) / 2.0));
        let anchor = self.point(effect, "anchor", (f64::from(source.0) / 2.0, f64::from(source.1) / 2.0));
        let fit =
            if clip.scale_to_frame { (f64::from(frame.0) / f64::from(source.0.max(1))).min(f64::from(frame.1) / f64::from(source.1.max(1))) } else { 1.0 };
        let uniform = boolean(effect, "uniform_scale", true);
        let sy = self.scalar(effect, "scale", 100.0) / 100.0;
        let sx = if uniform { sy } else { self.scalar(effect, "scale_width", 100.0) / 100.0 };
        let sx = self.number(sx * fit, 1.0);
        let sy = self.number(sy * fit, 1.0);
        for (name, value) in [
            ("x", position.0),
            ("y", position.1),
            ("anchorX", anchor.0),
            ("anchorY", anchor.1),
            ("scaleX", sx),
            ("scaleY", sy),
            ("rotation", self.scalar(effect, "rotation", 0.0)),
        ] {
            out.attr(name, value)?;
        }
        let opacity = clip.effect("opacity").filter(|e| e.enabled);
        out.attr("opacity", (self.scalar(opacity, "opacity", 100.0) / 100.0).clamp(0.0, 1.0))?;
        if self.scalar(opacity, "blend", 0.0) != 0.0 {
            self.report.warn("blend mode replaced with normal");
        }
        if self.scalar(effect, "anti_flicker", 0.0) != 0.0 {
            self.report.warn("anti-flicker filter dropped");
        }
        Ok(())
    }

    fn animation(
        &mut self,
        out: &mut Xml,
        param: Option<&Param>,
        property: &str,
        component: Option<bool>,
        conversion: (f64, f64),
        clip: Option<&TrackItem>,
    ) -> Result<()> {
        let (factor, offset) = conversion;
        let Some(param) = param.filter(|p| !p.keyframes.is_empty()) else {
            return Ok(());
        };
        self.spend(param.keyframes.len())?;
        let mut keys = Xml::default();
        let mut previous = None;
        let reverse = clip.is_some_and(|c| c.reverse);
        for order in 0..param.keyframes.len() {
            let index = if reverse { param.keyframes.len().saturating_sub(order).saturating_sub(1) } else { order };
            let key = param.keyframes.get(index).ok_or_else(|| failure("missing animation key"))?;
            let reverse_hold = reverse && index.checked_sub(1).and_then(|i| param.keyframes.get(i)).is_some_and(|k| k.interp == Interpolation::Hold);
            let key_time = clip.map_or(Ok(key.time), |c| animation_time(c, key.time, reverse_hold))?;
            let value = match component {
                Some(y) => key.value.as_vec2().map(|p| if y { p.y } else { p.x }),
                None => key.value.as_f64(),
            };
            let Some(value) = value.filter(|v| v.is_finite()).map(|v| v * factor + offset).filter(|v| v.is_finite()) else {
                self.report.warn("non-finite or unsupported animation key dropped");
                continue;
            };
            if previous.is_some_and(|t| t >= key_time) {
                self.report.warn("non-increasing animation key dropped");
                continue;
            }
            let Ok(time) = seconds(key_time) else {
                self.report.warn("unrepresentable animation time dropped");
                continue;
            };
            let bounded = match property {
                "opacity" | "volume" => value.clamp(0.0, 1.0),
                "gain" => value.clamp(-120.0, 24.0),
                _ => value,
            };
            if bounded != value {
                self.report.warn("animation value clipped to the scene property range");
            }
            let value = bounded;
            previous = Some(key_time);
            keys.push("<key")?;
            keys.time("time", time)?;
            keys.attr("value", value)?;
            let (kind, points) = if reverse {
                index.checked_sub(1).and_then(|i| param.keyframes.get(i)).map_or(("linear", None), |previous| {
                    let (kind, points) = curve(previous, Some(key), self.report);
                    (kind, points.map(|[x1, y1, x2, y2]| [1.0 - x2, 1.0 - y2, 1.0 - x1, 1.0 - y1]))
                })
            } else {
                curve(key, param.keyframes.get(index.saturating_add(1)), self.report)
            };
            keys.attr("interpolation", kind)?;
            if let Some([x1, y1, x2, y2]) = points {
                keys.attr("bezier", format!("{x1},{y1},{x2},{y2}"))?;
            }
            keys.push("/>\n")?;
            if reverse
                && kind == "hold"
                && let Some(held) = index.checked_sub(1).and_then(|i| param.keyframes.get(i))
            {
                let time = add(key_time, Tick(1)).ok_or_else(|| failure("reversed hold boundary overflow"))?;
                let end = clip.map_or(Ok(held.time), |c| animation_time(c, held.time, false))?;
                if time < end {
                    let value = match component {
                        Some(y) => held.value.as_vec2().map(|p| if y { p.y } else { p.x }),
                        None => held.value.as_f64(),
                    }
                    .filter(|v| v.is_finite())
                    .map(|v| v * factor + offset)
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| failure("invalid reversed hold value"))?;
                    let value = match property {
                        "opacity" | "volume" => value.clamp(0., 1.),
                        "gain" => value.clamp(-120., 24.),
                        _ => value,
                    };
                    self.spend(1)?;
                    keys.push("<key")?;
                    keys.time("time", seconds(time)?)?;
                    keys.attr("value", value)?;
                    keys.attr("interpolation", "hold")?;
                    keys.push("/>\n")?;
                    previous = Some(time);
                    self.report.info("Reversed hold boundary baked to adjacent local ticks");
                }
            }
        }
        if !keys.text.is_empty() {
            out.push("<animate")?;
            out.attr("property", property)?;
            out.attr("timeBase", "local")?;
            out.push(">\n")?;
            out.append(keys)?;
            out.push("</animate>\n")?;
        }
        Ok(())
    }

    fn clip_animations(&mut self, out: &mut Xml, clip: &TrackItem, frame: (u32, u32), source: (u32, u32)) -> Result<()> {
        let effect = clip.effect("motion").filter(|e| e.enabled);
        let factor =
            if clip.scale_to_frame { (f64::from(frame.0) / f64::from(source.0.max(1))).min(f64::from(frame.1) / f64::from(source.1.max(1))) } else { 1.0 };
        for (param, property, component, scale) in [
            ("position", "x", Some(false), 1.0),
            ("position", "y", Some(true), 1.0),
            ("anchor", "anchorX", Some(false), 1.0),
            ("anchor", "anchorY", Some(true), 1.0),
            ("rotation", "rotation", None, 1.0),
            ("scale", "scaleY", None, factor / 100.0),
        ] {
            self.animation(out, effect.and_then(|e| e.param(param)), property, component, (scale, 0.0), Some(clip))?;
        }
        let sx = if boolean(effect, "uniform_scale", true) { "scale" } else { "scale_width" };
        self.animation(out, effect.and_then(|e| e.param(sx)), "scaleX", None, (factor / 100.0, 0.0), Some(clip))?;
        self.animation(out, clip.effect("opacity").filter(|e| e.enabled).and_then(|e| e.param("opacity")), "opacity", None, (0.01, 0.0), Some(clip))
    }

    fn picture(&mut self, out: &mut Xml, clip: &TrackItem, frame: (u32, u32), visible: bool) -> Result<Option<String>> {
        self.spend(1)?;
        self.unsupported(clip)?;
        let Some(end) = self.interval(clip.start, clip.duration) else {
            return Ok(None);
        };
        if !clip.speed.is_finite() || clip.source_in.0 < 0 {
            self.report.warn("invalid clip speed or source in-point skipped");
            return Ok(None);
        }
        let Some(base) = self.base(clip.item) else {
            return Ok(None);
        };
        let Some(item) = self.project.item(base) else {
            return Ok(None);
        };
        let source = match &item.kind {
            ItemKind::Sequence(s) => (s.settings.width, s.settings.height),
            ItemKind::Graphic { width, height, .. } => (*width, *height),
            ItemKind::Media(m) => match m.info.video.as_ref() {
                Some(v) => (v.width, v.height),
                None => {
                    self.report.warn("audio-only source on a picture track skipped");
                    return Ok(None);
                }
            },
            _ => {
                self.report.warn("adjustment layer or unsupported source skipped");
                return Ok(None);
            }
        };
        if filmcraft_project::validate_frame_size(source.0, source.1).is_err() {
            self.report.warn("invalid source dimensions skipped");
            return Ok(None);
        }
        let id = self.id();
        let mut node = Xml::default();
        let group = matches!(item.kind, ItemKind::Sequence(_) | ItemKind::Graphic { .. });
        let mut source_in = clip.source_in;
        let mut speed = clip.speed.abs();
        if let Some(hold) = clip.frame_hold {
            source_in = hold;
            speed = 0.0;
        }
        let span = if speed == 0.0 { Some(Tick::ZERO) } else { ratio(speed).and_then(|(n, d)| scale(clip.duration, n, d)) };
        let Some(source_out) = span.and_then(|s| add(source_in, s)).filter(|t| speed == 0.0 || *t > source_in) else {
            self.report.warn("unrepresentable source span skipped");
            return Ok(None);
        };
        if seconds(source_in).is_err() || seconds(source_out).is_err() {
            self.report.warn("unrepresentable source time skipped");
            return Ok(None);
        }
        let linked = if self.building.len() == 1 { self.layer_audio.get(&clip.id).copied() } else { None };
        let usage = if linked.is_some() { AssetUse::Linked } else { AssetUse::Picture };
        let asset = if !group { self.asset(base, usage)? } else { None };
        if !group && asset.is_none() {
            return Ok(None);
        }
        node.push(if group { "<group" } else { "<layer" })?;
        node.attr("id", &id)?;
        node.attr("name", &clip.name)?;
        node.time("start", seconds(clip.start)?)?;
        node.time("end", seconds(end)?)?;
        node.attr("visible", visible && clip.enabled)?;
        self.motion(&mut node, clip, frame, source)?;
        if let Some(asset) = asset {
            node.attr("asset", asset)?;
            node.time("clipIn", seconds(source_in)?)?;
            if speed != 0.0 {
                node.time("clipOut", seconds(source_out)?)?;
            }
            node.attr("speed", speed)?;
            node.attr("reverse", clip.reverse ^ (clip.speed < 0.0))?;
            node.attr("mute", linked.is_none_or(|audio| audio.muted))?;
            if let Some(audio) = linked {
                node.attr("volume", audio.volume)?;
            }
            node.attr(
                "frameBlend",
                match clip.time_interpolation {
                    filmcraft_project::TimeInterpolation::FrameSampling => "none",
                    filmcraft_project::TimeInterpolation::FrameBlending => "frame-mix",
                    filmcraft_project::TimeInterpolation::OpticalFlow => "optical-flow",
                },
            )?;
            node.push(">\n")?;
            self.clip_animations(&mut node, clip, frame, source)?;
            node.push("</layer>\n")?;
        } else {
            node.attr("isolate", true)?;
            node.attr("width", source.0)?;
            node.attr("height", source.1)?;
            node.attr("clip", true)?;
            if let ItemKind::Sequence(seq) = &item.kind {
                if self.building.contains(&base) || self.building.len() >= MAX_DEPTH {
                    self.report.warn("nested sequence cycle or depth limit: clip skipped");
                    return Ok(None);
                }
                if seq.multicam.is_some() || clip.multicam.is_some() || speed == 0.0 || clip.reverse ^ (clip.speed < 0.0) {
                    self.report.warn("multicam, reversed or held nested sequence skipped");
                    return Ok(None);
                }
                let offset = ratio(speed).and_then(|(n, d)| clip.start.0.checked_sub(source_in.0).and_then(|v| scale(Tick(v), d, n)));
                let Some(offset) = offset.and_then(|t| seconds(t).ok()) else {
                    self.report.warn("unrepresentable nested clock skipped");
                    return Ok(None);
                };
                node.time("timeOffset", offset)?;
                node.attr("timeScale", speed)?;
                node.push(">\n")?;
                self.clip_animations(&mut node, clip, frame, source)?;
                self.building.push(base);
                self.sequence(&mut node, seq)?;
                self.building.pop();
                for _ in &seq.caption_tracks {
                    self.report.warn("nested caption track dropped");
                }
                for _ in &seq.markers {
                    self.report.warn("nested sequence marker dropped");
                }
            } else {
                node.push(">\n")?;
                self.clip_animations(&mut node, clip, frame, source)?;
                for effect in clip.graphic_layers() {
                    self.graphic(&mut node, effect, clip.start, end, source)?;
                }
            }
            node.push("</group>\n")?;
        }
        out.append(node)?;
        if let Some(audio) = linked {
            self.embedded_audio.insert(audio.clip);
        }
        Ok(Some(id))
    }

    fn sequence(&mut self, out: &mut Xml, seq: &Sequence) -> Result<()> {
        self.spend(seq.video_tracks.len().saturating_add(seq.audio_tracks.len()).saturating_add(seq.caption_tracks.len()).saturating_add(seq.markers.len()))?;
        if self.building.len() == 1 {
            self.prepare_layer_audio(seq)?;
        }
        if seq.multicam.is_some() || seq.merged.is_some() {
            self.report.warn("multicam or merged sequence metadata dropped");
        }
        if seq.settings.par != (1, 1) {
            self.report.warn("non-square sequence pixel aspect ratio dropped");
        }
        self.spend(seq.master_effects.len().saturating_add(seq.submix_tracks.len()))?;
        for _ in &seq.master_effects {
            self.report.warn("master effect dropped");
        }
        for _ in &seq.submix_tracks {
            self.report.warn("audio submix dropped");
        }
        if seq.master_mixer != Default::default() {
            self.report.warn("master mixer automation/routing dropped");
        }
        let frame = (seq.settings.width, seq.settings.height);
        for track in &seq.video_tracks {
            self.spend(track.items.len().saturating_add(track.transitions.len()))?;
            let mut ids = BTreeMap::new();
            for clip in &track.items {
                if ids.contains_key(&clip.id) {
                    self.report.warn("duplicate clip id skipped");
                    continue;
                }
                if let Some(id) = self.picture(out, clip, frame, track.enabled)? {
                    ids.insert(clip.id, id);
                }
            }
            self.transitions(out, track, &ids)?;
        }
        Ok(())
    }

    fn transitions(&mut self, out: &mut Xml, track: &Track, ids: &BTreeMap<ClipId, String>) -> Result<()> {
        for tr in &track.transitions {
            if !tr.effect.enabled {
                continue;
            }
            let from = tr.from.and_then(|c| ids.get(&c));
            let to = tr.to.and_then(|c| ids.get(&c));
            if (tr.from.is_some() && from.is_none()) || (tr.to.is_some() && to.is_none()) || (from.is_none() && to.is_none()) || tr.duration.0 <= 0 {
                self.report.warn("transition with missing sides or invalid duration skipped");
                continue;
            }
            let Ok(duration) = seconds(tr.duration) else {
                self.report.warn("unrepresentable transition duration skipped");
                continue;
            };
            let cut = tr
                .from
                .and_then(|id| track.items.iter().find(|c| c.id == id))
                .and_then(|c| add(c.start, c.duration))
                .or_else(|| tr.to.and_then(|id| track.items.iter().find(|c| c.id == id)).map(|c| c.start));
            let offset = match tr.align {
                TransitionAlign::CenterAtCut => tr.duration.0 / 2,
                TransitionAlign::StartAtCut => 0,
                TransitionAlign::EndAtCut => tr.duration.0,
            };
            if cut.and_then(|c| c.0.checked_sub(offset)).is_none_or(|start| start != tr.start.0) {
                self.report.warn("transition window differs from sibling cut; transition skipped");
                continue;
            }
            let kind = match tr.effect.effect.as_str() {
                "cross_dissolve" | "cross_dissolve_legacy" => "crossfade",
                "additive_dissolve" => "additive-dissolve",
                "dip_to_black" | "dip_to_white" | "dip_to_color" => "dip-to-color",
                _ => {
                    self.report.warn("unknown transition replaced with cross dissolve");
                    "crossfade"
                }
            };
            self.minor = self.minor.max(1);
            out.push("<transition")?;
            out.attr("type", kind)?;
            if let Some(id) = from {
                out.attr("from", id)?;
            }
            if let Some(id) = to {
                out.attr("to", id)?;
            }
            out.time("duration", duration)?;
            out.attr(
                "alignment",
                match tr.align {
                    TransitionAlign::CenterAtCut => "center",
                    TransitionAlign::StartAtCut => "start",
                    TransitionAlign::EndAtCut => "end",
                },
            )?;
            out.attr("curve", "linear")?;
            if tr.effect.effect == "dip_to_white" {
                out.attr("color", "#FFFFFFFF")?;
            }
            if tr.effect.effect == "dip_to_color" {
                out.attr("color", self.color(Some(&tr.effect), "color"))?;
            }
            out.push("/>\n")?;
            if tr.reverse || tr.effect.is_animated() || !tr.effect.masks.is_empty() {
                self.report.warn("transition reverse, animation or masks dropped");
            }
        }
        Ok(())
    }

    fn color(&mut self, effect: Option<&EffectInstance>, param: &str) -> String {
        let [r, g, b, a] = effect.and_then(|e| e.param(param)).and_then(|p| p.value.as_color()).unwrap_or([1.0; 4]);
        let channel = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        if [r, g, b, a].iter().any(|c| !c.is_finite() || !(0.0..=1.0).contains(c)) {
            self.report.warn("invalid colour channels replaced or clipped");
        }
        format!("{},{},{},{}", channel(r), channel(g), channel(b), channel(a))
    }

    fn graphic(&mut self, out: &mut Xml, effect: &EffectInstance, start: Tick, end: Tick, frame: (u32, u32)) -> Result<()> {
        self.spend(1)?;
        if !effect.enabled {
            return Ok(());
        }
        let text = effect.effect == graphic::TEXT_LAYER;
        let path_shape = !text && self.scalar(Some(effect), "shape", 0.0) == 3.0;
        let id = self.id();
        let mut node = Xml::default();
        let size = if text {
            let w = self.scalar(Some(effect), "box_width", 0.0);
            let h = self.scalar(Some(effect), "box_height", 0.0);
            if w <= 0.0 || h <= 0.0 {
                self.report.warn("point text uses a frame-sized text box");
            }
            (if w > 0.0 { w } else { f64::from(frame.0) }, if h > 0.0 { h } else { f64::from(frame.1) })
        } else {
            self.point(Some(effect), "size", (400.0, 200.0))
        };
        if size.0 <= 0.0 || size.1 <= 0.0 || size.0 > 32768.0 || size.1 > 32768.0 {
            self.report.warn("invalid graphic bounds skipped");
            return Ok(());
        }
        if text {
            let content = string(effect, "text");
            if content.is_empty() {
                self.report.warn("empty text layer skipped");
                return Ok(());
            }
            let mut asset = Xml::default();
            asset.push("<text")?;
            asset.attr("id", &id)?;
            asset.attr("text", content)?;
            asset.attr("width", size.0.ceil() as u32)?;
            asset.attr("height", size.1.ceil() as u32)?;
            asset.attr("size", self.scalar(Some(effect), "size", 100.0).clamp(0.01, 4096.0))?;
            asset.attr("font", string(effect, "font"))?;
            asset.attr(
                "align",
                match self.scalar(Some(effect), "align", 0.0) {
                    1.0 => "center",
                    2.0 => "end",
                    3.0 => "justify",
                    _ => "start",
                },
            )?;
            asset.attr("color", if boolean(Some(effect), "fill", true) { self.color(Some(effect), "fill_color") } else { "0,0,0,0".into() })?;
            asset.push("/>\n")?;
            self.assets.append(asset)?;
            node.push("<layer")?;
            node.attr("asset", id)?;
        } else {
            node.push("<shape")?;
            let shape = match self.scalar(Some(effect), "shape", 0.0) {
                0.0 => "rect",
                1.0 => "ellipse",
                2.0 => "polygon",
                3.0 => "path",
                _ => {
                    self.report.warn("unsupported shape path skipped");
                    return Ok(());
                }
            };
            node.attr("shape", shape)?;
            if path_shape {
                let points = effect.param("points").and_then(|p| p.value.as_curve()).unwrap_or(&[]);
                self.spend(points.len())?;
                if points.len() < 3 || points.iter().any(|p| p.iter().any(|v| !v.is_finite())) {
                    self.report.warn("invalid graphic path skipped");
                    return Ok(());
                }
                let mut path = BoundedValue(String::new());
                for (i, [x, y]) in points.iter().enumerate() {
                    write!(&mut path, "{}{x},{y}", if i == 0 { "M" } else { " L" }).map_err(|_| failure("graphic path exceeds attribute budget"))?;
                }
                path.write_str(" Z").map_err(|_| failure("graphic path exceeds attribute budget"))?;
                node.attr("path", path.0)?;
            }
            node.attr("width", size.0)?;
            node.attr("height", size.1)?;
            node.attr("radius", self.scalar(Some(effect), "corner_radius", 0.0).max(0.0))?;
            node.attr("points", self.scalar(Some(effect), "sides", 6.0).clamp(3.0, 64.0).round() as u32)?;
            node.attr("fill", if boolean(Some(effect), "fill", true) { self.color(Some(effect), "fill_color") } else { "0,0,0,0".into() })?;
            if boolean(Some(effect), "stroke", false) {
                node.attr("stroke", self.color(Some(effect), "stroke_color"))?;
                node.attr("strokeWidth", self.scalar(Some(effect), "stroke_width", 0.0).max(0.0))?;
            }
        }
        node.attr("id", self.id())?;
        node.attr("name", string(effect, "name"))?;
        node.time("start", seconds(start)?)?;
        node.time("end", seconds(end)?)?;
        let position = self.point(Some(effect), "position", (f64::from(frame.0) / 2.0, f64::from(frame.1) / 2.0));
        let anchor = self.point(Some(effect), "anchor", (0.0, 0.0));
        // FilmCraft shapes are centred at their local origin; scene shapes occupy [0,w] × [0,h].
        let centered = !text && !path_shape;
        let anchor = if centered { (anchor.0 + size.0 / 2.0, anchor.1 + size.1 / 2.0) } else { anchor };
        let sy = self.scalar(Some(effect), "scale", 100.0) / 100.0;
        let sx = if boolean(Some(effect), "uniform_scale", true) { sy } else { self.scalar(Some(effect), "scale_width", 100.0) / 100.0 };
        for (name, value) in [
            ("x", position.0),
            ("y", position.1),
            ("anchorX", anchor.0),
            ("anchorY", anchor.1),
            ("scaleX", sx),
            ("scaleY", sy),
            ("rotation", self.scalar(Some(effect), "rotation", 0.0)),
            ("opacity", (self.scalar(Some(effect), "opacity", 100.0) / 100.0).clamp(0.0, 1.0)),
        ] {
            node.attr(name, value)?;
        }
        node.push(">\n")?;
        for (param, property, component, factor, offset) in [
            ("position", "x", Some(false), 1.0, 0.0),
            ("position", "y", Some(true), 1.0, 0.0),
            ("anchor", "anchorX", Some(false), 1.0, if centered { size.0 / 2.0 } else { 0.0 }),
            ("anchor", "anchorY", Some(true), 1.0, if centered { size.1 / 2.0 } else { 0.0 }),
            ("rotation", "rotation", None, 1.0, 0.0),
            ("scale", "scaleY", None, 0.01, 0.0),
            ("opacity", "opacity", None, 0.01, 0.0),
        ] {
            self.animation(&mut node, effect.param(param), property, component, (factor, offset), None)?;
        }
        let sx = if boolean(Some(effect), "uniform_scale", true) { "scale" } else { "scale_width" };
        self.animation(&mut node, effect.param(sx), "scaleX", None, (0.01, 0.0), None)?;
        node.push(if text { "</layer>\n" } else { "</shape>\n" })?;
        out.append(node)?;
        for (name, param) in &effect.params {
            if param.is_animated() && !matches!(name.as_str(), "position" | "anchor" | "rotation" | "scale" | "scale_width" | "opacity") {
                self.report.warn("graphic content or appearance animation dropped");
            }
        }
        if effect.layer.is_some() {
            self.report.warn("graphic style runs and pinning dropped");
        }
        for param in ["stroke2", "background", "shadow", "faux_bold", "faux_italic", "underline", "vertical"] {
            if boolean(Some(effect), param, false) {
                self.report.warn("graphic appearance option dropped");
            }
        }
        for param in ["fill_kind", "tracking", "leading", "baseline_shift", "caps"] {
            if self.scalar(Some(effect), param, 0.0) != 0.0 {
                self.report.warn("graphic typography or gradient option dropped");
            }
        }
        Ok(())
    }

    fn prepare_layer_audio(&mut self, seq: &Sequence) -> Result<()> {
        let mut ids = BTreeSet::new();
        let mut transition_sides = BTreeSet::new();
        for track in seq.all_tracks() {
            self.spend_work(track.items.len().saturating_add(track.transitions.len()))?;
            for clip in &track.items {
                if !ids.insert(clip.id) {
                    return Err(failure("duplicate clip id in sequence"));
                }
            }
            for transition in &track.transitions {
                if transition.effect.enabled {
                    transition_sides.extend(transition.from.into_iter().chain(transition.to));
                }
            }
        }
        let mut pictures: BTreeMap<u64, Option<&TrackItem>> = BTreeMap::new();
        let mut sounds: BTreeMap<u64, Option<(&TrackItem, &Track)>> = BTreeMap::new();
        for track in &seq.video_tracks {
            self.spend_work(track.items.len())?;
            for clip in &track.items {
                if let Some(link) = clip.link {
                    pictures.entry(link).and_modify(|pair| *pair = None).or_insert(track.enabled.then_some(clip));
                }
            }
        }
        for track in &seq.audio_tracks {
            self.spend_work(track.items.len())?;
            for clip in &track.items {
                if let Some(link) = clip.link {
                    sounds.entry(link).and_modify(|pair| *pair = None).or_insert(Some((clip, track)));
                }
            }
        }
        let solo = seq.audio_tracks.iter().any(|track| track.solo);
        for (link, picture) in pictures {
            let (Some(picture), Some(Some((sound, track)))) = (picture, sounds.get(&link)) else {
                continue;
            };
            // Scene transitions add audio handles and default crossfade gains to their layers.
            // Until native audio transitions can be matched, keep these placements independent.
            if transition_sides.contains(&picture.id) || transition_sides.contains(&sound.id) {
                continue;
            }
            let Some(end) = window(picture.start, picture.duration) else {
                continue;
            };
            // sr-deliver samples layer clocks on frame boundaries; sr-audio extrapolates
            // their final frame at forward 1x. Other clocks need independent placements.
            if !safe_audio_frame_boundary(picture.start, seq.settings.frame_rate)
                || !safe_audio_frame_boundary(end, seq.settings.frame_rate)
                || picture.speed != 1.0
                || picture.reverse
            {
                continue;
            }
            // layerType has one media clock, linear volume <= 1, and no pan or general edit link.
            if picture.item != sound.item
                || picture.start != sound.start
                || picture.duration != sound.duration
                || picture.source_in != sound.source_in
                || picture.speed != sound.speed
                || picture.reverse != sound.reverse
                || picture.frame_hold.is_some()
                || sound.frame_hold.is_some()
                || !picture.enabled
                || sound.audio_stream != 0
                || !sound.source_channels.is_empty()
                || track.pan != 0.0
            {
                continue;
            }
            let Some(base) = self.base(picture.item) else {
                continue;
            };
            if !self
                .project
                .item(base)
                .and_then(|item| item.as_media())
                .is_some_and(|media| media.info.kind != MediaKind::Still && media.info.video.is_some() && media.info.has_audio())
            {
                continue;
            }
            self.spend_work(sound.effects.len())?;
            let effect = sound.effect("volume").filter(|e| e.enabled && !boolean(Some(e), "bypass", false));
            let level = effect.and_then(|e| e.param("level"));
            if level.is_some_and(|p| p.is_animated()) {
                continue;
            }
            let level = level.and_then(|p| p.value.as_f64()).unwrap_or(0.0);
            if ![sound.gain_db, track.volume_db, seq.master_volume_db, level].into_iter().all(f64::is_finite) {
                continue;
            }
            // Import stores layer mute in clip gain; keep its independent volume while muted.
            let gain = if sound.gain_db <= -120.0 { 0.0 } else { sound.gain_db };
            let gain = gain + track.volume_db + seq.master_volume_db + level;
            if !gain.is_finite() || gain > 0.0 {
                continue;
            }
            let volume = if gain <= -120.0 { 0.0 } else { 10_f64.powf(gain / 20.0) };
            let muted = sound.gain_db <= -120.0 || !sound.enabled || !track.enabled || track.muted || (solo && !track.solo);
            self.layer_audio.insert(picture.id, LayerAudio { clip: sound.id, volume, muted });
        }
        Ok(())
    }

    fn audio(&mut self, seq: &Sequence) -> Result<Xml> {
        let mut out = Xml::default();
        let clock = AudioClock { low: Tick::ZERO, high: Tick(MAX_TIME), root: Tick::ZERO, n: 1, d: 1, gain: 0.0, muted: false };
        self.audio_sequence(seq, clock, &mut out)?;
        Ok(out)
    }

    fn audio_sequence(&mut self, seq: &Sequence, clock: AudioClock, out: &mut Xml) -> Result<()> {
        self.spend(seq.audio_tracks.len())?;
        let solo = seq.audio_tracks.iter().any(|t| t.solo);
        for track in &seq.audio_tracks {
            self.spend(track.items.len().saturating_add(track.transitions.len()).saturating_add(track.effects.len()))?;
            for _ in &track.effects {
                self.report.warn("audio track effect dropped");
            }
            for _ in &track.transitions {
                self.report.warn("audio track transition dropped");
            }
            if track.mixer != Default::default() {
                self.report.warn("audio track mixer automation/routing dropped");
            }
            let strip_gain = clock.gain + self.number(track.volume_db, 0.0) + self.number(seq.master_volume_db, 0.0);
            for clip in &track.items {
                self.unsupported(clip)?;
                if self.building.len() == 1 && self.embedded_audio.contains(&clip.id) {
                    continue;
                }
                let Some(end) = self.interval(clip.start, clip.duration) else {
                    continue;
                };
                if clip.source_in.0 < 0 {
                    self.report.warn("negative audio source time skipped");
                    continue;
                }
                if clip.frame_hold.is_some() || clip.speed == 0.0 {
                    self.report.warn("held audio clip omitted as silence");
                    continue;
                }
                let Some((n, d)) = ratio(clip.speed.abs()) else {
                    self.report.warn("invalid audio speed skipped");
                    continue;
                };
                let low = clip.start.max(clock.low);
                let high = end.min(clock.high);
                if high <= low {
                    continue;
                }
                let reverse = clip.reverse ^ (clip.speed < 0.0);
                let source_low = if reverse { end.0.checked_sub(high.0) } else { low.0.checked_sub(clip.start.0) };
                let source_high = if reverse { end.0.checked_sub(low.0) } else { high.0.checked_sub(clip.start.0) };
                let source_low = source_low.and_then(|t| scale(Tick(t), n, d)).and_then(|t| add(clip.source_in, t));
                let source_high = source_high.and_then(|t| scale(Tick(t), n, d)).and_then(|t| add(clip.source_in, t));
                let root = low.0.checked_sub(clock.low.0).and_then(|t| scale(Tick(t), clock.d, clock.n)).and_then(|t| add(clock.root, t));
                let speed = combine_ratio(n, d, clock.n, clock.d);
                let (Some(source_low), Some(source_high), Some(root), Some((n, d))) = (source_low, source_high, root, speed) else {
                    self.report.warn("unrepresentable audio clock skipped");
                    continue;
                };
                if source_high <= source_low {
                    self.report.warn("zero-length audio source span skipped");
                    continue;
                }
                let effect = clip.effect("volume").filter(|e| e.enabled && !boolean(Some(e), "bypass", false));
                let base_gain = strip_gain + self.number(clip.gain_db, 0.0);
                let level = self.scalar(effect, "level", 0.0);
                let gain = self.number(base_gain + level, 0.0);
                let muted = clock.muted || !clip.enabled || !track.enabled || track.muted || (solo && !track.solo);
                let Some(base) = self.base(clip.item) else {
                    continue;
                };
                if let Some(nested) = self.project.sequence(base) {
                    if self.building.contains(&base) || self.building.len() >= MAX_DEPTH {
                        self.report.warn("nested audio sequence cycle or depth limit: clip skipped");
                        continue;
                    }
                    if reverse || nested.multicam.is_some() || clip.multicam.is_some() {
                        self.report.warn("reversed or multicam nested audio skipped");
                        continue;
                    }
                    if effect.is_some_and(|e| e.is_animated()) {
                        self.report.warn("nested audio container gain animation dropped");
                    }
                    self.building.push(base);
                    self.audio_sequence(nested, AudioClock { low: source_low, high: source_high, root, n, d, gain, muted }, out)?;
                    self.building.pop();
                    continue;
                }
                let (Ok(start), Ok(input), Ok(output)) = (seconds(root), seconds(source_low), seconds(source_high)) else {
                    self.report.warn("unrepresentable audio time skipped");
                    continue;
                };
                if !self.project.item(base).and_then(|i| i.as_media()).is_some_and(|m| m.info.has_audio()) {
                    self.report.warn("audio clip without a file audio source skipped");
                    continue;
                }
                let speed = n as f64 / d as f64;
                if !speed.is_finite() || speed < 0.001 {
                    self.report.warn("audio speed below the scene minimum skipped");
                    continue;
                }
                let Some(asset) = self.asset(base, AssetUse::Audio)? else {
                    continue;
                };
                if clip.link.is_some() {
                    self.report.warn("AV edit linkage not retained; audio kept as an independent audioMix track");
                }
                out.push("<audioTrack")?;
                out.attr("id", self.id())?;
                out.attr("asset", asset)?;
                out.time("start", start)?;
                out.time("clipIn", input)?;
                out.time("clipOut", output)?;
                out.attr("speed", speed)?;
                out.attr("preservePitch", false)?;
                out.attr("reverse", reverse)?;
                if gain > 24.0 {
                    self.report.warn("audio gain clipped to scene's +24 dB limit");
                }
                out.attr("gain", gain.clamp(-120.0, 24.0))?;
                out.attr("volume", 1)?;
                out.attr("pan", (self.number(track.pan, 0.0) / 100.0).clamp(-1.0, 1.0))?;
                let animated = effect.and_then(|e| e.param("level")).is_some_and(|p| p.is_animated());
                out.attr("mute", muted || (gain <= -120.0 && !animated))?;
                out.push(">\n")?;
                if let Some(param) = effect.and_then(|e| e.param("level")) {
                    self.spend(param.keyframes.len())?;
                    let mut mapped = Param::new(ParamValue::Float(0.0));
                    for key in &param.keyframes {
                        let time = key.time.0.checked_sub(low.0 - clip.start.0).and_then(|t| scale(Tick(t), clock.d, clock.n));
                        if let (Some(time), Some(value)) = (time, key.value.as_f64()) {
                            mapped.keyframes.push(Keyframe {
                                time,
                                value: ParamValue::Float(value),
                                interp: key.interp,
                                out_influence: key.out_influence,
                                in_influence: key.in_influence,
                            });
                        } else {
                            self.report.warn("unrepresentable nested audio key skipped");
                        }
                    }
                    self.animation(out, Some(&mapped), "gain", None, (1.0, base_gain), None)?;
                }
                out.push("</audioTrack>\n")?;
            }
        }
        Ok(())
    }

    fn markers(&mut self, seq: &Sequence) -> Result<Xml> {
        let mut out = Xml::default();
        for marker in &seq.markers {
            let (Ok(time), Ok(duration)) = (seconds(marker.start), seconds(marker.duration)) else {
                self.report.warn("unrepresentable marker skipped");
                continue;
            };
            if marker.duration.0 < 0 {
                self.report.warn("negative marker duration skipped");
                continue;
            }
            self.minor = self.minor.max(1);
            out.push("<marker")?;
            out.attr("id", self.id())?;
            out.time("time", time)?;
            out.time("duration", duration)?;
            out.attr("label", &marker.name)?;
            out.attr(
                "kind",
                match marker.kind {
                    MarkerKind::Chapter => "chapter",
                    MarkerKind::Segmentation => "section",
                    _ => "comment",
                },
            )?;
            let [r, g, b] = marker.color.marker_rgb();
            out.attr("color", format!("#{r:02X}{g:02X}{b:02X}"))?;
            out.push("/>\n")?;
            if !marker.comment.is_empty() || matches!(marker.kind, MarkerKind::WebLink | MarkerKind::FlashCue) {
                self.report.warn("marker comment or interactive metadata dropped");
            }
        }
        Ok(out)
    }

    fn captions(&mut self, seq: &Sequence) -> Result<Xml> {
        let mut out = Xml::default();
        for track in &seq.caption_tracks {
            self.spend(track.captions.len())?;
            if !track.enabled {
                self.report.warn("disabled caption track dropped");
                continue;
            }
            let mut cues = Xml::default();
            for cue in &track.captions {
                let Some(end) = self.interval(cue.start, cue.duration) else {
                    continue;
                };
                cues.push("<cue")?;
                cues.time("start", seconds(cue.start)?)?;
                cues.time("end", seconds(end)?)?;
                cues.attr("text", &cue.text)?;
                if let Some(speaker) = &cue.speaker {
                    cues.attr("speaker", speaker)?;
                }
                cues.push("/>\n")?;
                if cue.cue_id.is_some() || !cue.settings.is_empty() {
                    self.report.warn("caption cue identifier/settings dropped");
                }
            }
            if cues.text.is_empty() {
                continue;
            }
            self.minor = self.minor.max(1);
            out.push("<captionTrack")?;
            out.attr("id", self.id())?;
            out.attr("label", &track.name)?;
            // An unspecified language is BCP-47 'und', rather than guessing the speaker's language.
            let language = if valid_language(&track.language) {
                track.language.as_str()
            } else {
                self.report.warn("invalid or unspecified caption language replaced with und");
                "und"
            };
            out.attr("language", language)?;
            out.attr("preset", "none")?;
            out.attr("lineBreaks", "source")?;
            out.push(">\n")?;
            out.append(cues)?;
            out.push("</captionTrack>\n")?;
            self.report.warn("caption track styling and delivery format dropped");
        }
        Ok(out)
    }
}

fn boolean(effect: Option<&EffectInstance>, param: &str, fallback: bool) -> bool {
    effect.and_then(|e| e.param(param)).and_then(|p| p.value.as_bool()).unwrap_or(fallback)
}
fn string<'a>(effect: &'a EffectInstance, param: &str) -> &'a str {
    match effect.param(param).map(|p| &p.value) {
        Some(ParamValue::Text(s)) => s,
        _ => "",
    }
}
fn modified(effect: &EffectInstance) -> bool {
    effect.is_animated() || effect.params.iter().any(|(name, p)| effect.def().and_then(|d| d.param(name)).is_none_or(|d| p.value != d.default))
}
fn valid_language(language: &str) -> bool {
    let mut parts = language.split('-');
    parts.next().is_some_and(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphabetic()))
        && parts.all(|p| !p.is_empty() && p.len() <= 8 && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

fn curve(key: &Keyframe, next: Option<&Keyframe>, report: &mut Report) -> (&'static str, Option<[f64; 4]>) {
    if key.interp == Interpolation::Hold {
        return ("hold", None);
    }
    let outgoing = matches!(key.interp, Interpolation::Bezier | Interpolation::AutoBezier | Interpolation::ContinuousBezier | Interpolation::EaseOut);
    let incoming = next
        .is_some_and(|k| matches!(k.interp, Interpolation::Bezier | Interpolation::AutoBezier | Interpolation::ContinuousBezier | Interpolation::EaseIn))
        || matches!(key.interp, Interpolation::Bezier | Interpolation::ContinuousBezier);
    if !outgoing && !incoming {
        return ("linear", None);
    }
    let mut influence = |v: f64| {
        if v.is_finite() {
            v.clamp(0.01, 1.0)
        } else {
            report.warn("non-finite keyframe influence replaced with one third");
            1.0 / 3.0
        }
    };
    let x1 = if outgoing { influence(key.out_influence) } else { 1.0 / 3.0 };
    let y1 = if outgoing { 0.0 } else { 1.0 / 3.0 };
    let x2 = if incoming { 1.0 - influence(next.map_or(1.0 / 3.0, |k| k.in_influence)) } else { 2.0 / 3.0 };
    let y2 = if incoming { 1.0 } else { 2.0 / 3.0 };
    ("cubic-bezier", Some([x1, y1, x2, y2]))
}

/// Export a sequence without I/O, reporting content the scene format cannot carry.
pub(crate) fn export(p: &Project, seq_id: ItemId, opts: &ExportOptions, report: &mut Report) -> Result<String> {
    let seq = p.sequence(seq_id).ok_or(Error::NoSequence(seq_id))?;
    filmcraft_project::validate_frame_size(seq.settings.width, seq.settings.height).map_err(|_| failure("invalid sequence frame size"))?;
    let fps = rate(seq.settings.frame_rate)?;
    let mut x = Exporter {
        project: p,
        opts,
        report,
        assets: Xml::default(),
        asset_ids: BTreeMap::new(),
        layer_audio: BTreeMap::new(),
        embedded_audio: BTreeSet::new(),
        building: vec![seq_id],
        next: 0,
        remaining: MAX_NODES,
        remaining_work: MAX_WORK,
        minor: 0,
    };
    let mut body = Xml::default();
    x.sequence(&mut body, seq)?;
    let audio = x.audio(seq)?;
    let markers = x.markers(seq)?;
    let captions = x.captions(seq)?;
    let mut duration = Tick::ZERO;
    for track in seq.all_tracks() {
        for clip in &track.items {
            if let Some(end) = window(clip.start, clip.duration) {
                duration = duration.max(end);
            }
        }
    }
    for track in &seq.caption_tracks {
        for cue in &track.captions {
            if let Some(end) = window(cue.start, cue.duration) {
                duration = duration.max(end);
            }
        }
    }
    if duration == Tick::ZERO {
        let frame = i128::from(TICKS_PER_SECOND) * i128::from(seq.settings.frame_rate.den) / i128::from(seq.settings.frame_rate.num);
        duration = Tick(i64::try_from(frame).map_err(|_| failure("unrepresentable frame duration"))?);
        x.report.info("empty sequence exported with one frame duration");
    }
    let mut out = Xml::default();
    out.push("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<scene")?;
    out.attr("version", format!("1.{}", x.minor))?;
    out.push(">\n<project")?;
    out.attr("width", seq.settings.width)?;
    out.attr("height", seq.settings.height)?;
    out.attr("fps", fps)?;
    out.time("duration", seconds(duration)?)?;
    out.push("/>\n")?;
    if !x.assets.text.is_empty() {
        out.push("<assets>\n")?;
        out.append(x.assets)?;
        out.push("</assets>\n")?;
    }
    if !markers.text.is_empty() {
        out.push("<markers>\n")?;
        out.append(markers)?;
        out.push("</markers>\n")?;
    }
    out.push("<composition>\n")?;
    out.append(body)?;
    out.push("</composition>\n")?;
    if !audio.text.is_empty() {
        out.push("<audioMix")?;
        out.attr("sampleRate", seq.settings.sample_rate.max(1))?;
        out.push(">\n")?;
        out.append(audio)?;
        out.push("</audioMix>\n")?;
    }
    if !captions.text.is_empty() {
        out.push("<captions>\n")?;
        out.append(captions)?;
        out.push("</captions>\n")?;
    }
    out.push("</scene>\n")?;
    if out.repaired > 0 {
        x.report.warn("XML-invalid control characters replaced with U+FFFD");
    }
    let doc = read::read(&out.text, &Limits::default())?;
    let errors: Vec<_> = validate::validate(&doc.doc).into_iter().filter(|d| d.is_error()).collect();
    if !errors.is_empty() {
        return Err(Error::Invalid(errors));
    }
    if out.inexact_times > 0 {
        x.report.warn(format!(
            "{} time(s) beyond exact f64 tick precision written to the nearest representable value (max error {} ticks)",
            out.inexact_times, out.max_time_error
        ));
    }
    Ok(out.text)
}

#[cfg(test)]
mod tests;
