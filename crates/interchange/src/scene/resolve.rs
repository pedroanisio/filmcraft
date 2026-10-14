//! Semantic resolution of validated sr-core 1.7 scenes. All times remain seconds until mapping.
//!
//! Clock, sequence placement, transition and transform-parent semantics are adapted from scene-render
//! `crates/sr-eval/src/program.rs` and `eval.rs` (fit boxes: `layout.rs`), commit `9ebe0889`.

#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use super::{
    SceneDoc, err, validate,
    values::{self, Paint},
};
use crate::Result;
use filmcraft_time::FrameRate;
use roxmltree::{Node, NodeId};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Semantic expansion budgets, independent of parser and validation limits.
#[derive(Clone, Copy, Debug)]
pub struct ResolveLimits {
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_segments: usize,
    pub max_keys: usize,
    pub max_notes: usize,
}
impl Default for ResolveLimits {
    fn default() -> Self {
        Self { max_nodes: 100_000, max_depth: 128, max_segments: 100_000, max_keys: 100_000, max_notes: 100_000 }
    }
}

/// A half-open window in composition seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interval {
    pub start: f64,
    pub end: f64,
}

/// One affine conversion: composition seconds = offset + scale * timeline seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    pub offset: f64,
    pub scale: f64,
}

/// Resolved project settings; duration includes trailing blank time.
#[derive(Clone, Debug)]
pub struct ProjectSettings {
    pub width: u32,
    pub height: u32,
    pub fps: FrameRate,
    pub duration: f64,
    pub background: Paint,
}

/// Native mapping categories.
///
/// | Class | XSD elements |
/// |---|---|
/// | Media | layer referencing video, image, audio, imageSequence |
/// | Text | layer referencing text |
/// | Shape | shape: rect, rounded-rect, ellipse, polygon, star, line, path |
/// | Group | group (isolating/retimed or flattenable) |
/// | Sequence | sequence |
/// | Transition | transition (also resolved separately) |
/// | Unmappable | connector, object3D, camera, particleEmitter, particles3D, ocean, blackHole,
///   accretionDisk, flock, fluid, slime, erosion, instance, include, repeat, adjustment, skeleton,
///   compute, iterate, program, viewport3D; unknown nodes/assets/shapes and threeD nodes |
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeClass {
    Media(MediaKind),
    Text,
    Shape(String),
    Group { isolated: bool, retimed: bool },
    Sequence,
    Transition,
    Unmappable,
}

/// Asset kinds that the importer can represent as media.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Image,
    Audio,
    ImageSequence,
}

/// A constant-rate source interval. Rate is source seconds per composition second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceSegment {
    pub comp_start: f64,
    pub comp_end: f64,
    pub src_start: f64,
    pub rate: f64,
}

/// A remap key with source seconds and resolved composition time. No clipIn is added.
#[derive(Clone, Debug)]
pub struct RemapKey {
    pub xml: NodeId,
    pub comp_time: f64,
    pub source_time: f64,
    pub interpolation: String,
}

/// Media timing after the container clock chain is resolved.
#[derive(Clone, Debug)]
pub enum SourceMap {
    Segments(Vec<SourceSegment>),
    /// freeze_local, when present, selects a fixed local time on this remap curve.
    Remap {
        keys: Vec<RemapKey>,
        freeze_local: Option<f64>,
    },
}

/// Six coefficients: x' = a*x + c*y + e; y' = b*x + d*y + f.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

/// Static local transform parameters in output pixels (angles in degrees).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub anchor_x: f64,
    pub anchor_y: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotation: f64,
    pub skew_x: f64,
    pub skew_y: f64,
}

/// Layout frame and static transform. Unknown natural media dimensions remain explicit.
#[derive(Clone, Debug)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    pub size_known: bool,
    pub transform: Transform,
    pub local_matrix: Affine,
    /// Whether the local matrix and layout frame are independent of time and unsupported drivers.
    pub transform_static: bool,
    pub world_matrix: Option<Affine>,
    pub parent_transform_unresolved: bool,
}

/// Time base declared on an animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeBase {
    Composition,
    Local,
    Normalized,
}

/// One animation channel, retaining XML handles and resolved key times for the importer.
#[derive(Clone, Debug)]
pub struct Animation {
    pub xml: NodeId,
    pub target: NodeId,
    pub property: String,
    pub time_base: TimeBase,
    pub keys: Vec<(NodeId, f64)>,
    pub default_interpolation: String,
}

/// A composition node. `index` is stable document preorder; the returned nodes are in paint order.
#[derive(Clone, Debug)]
pub struct ResolvedNode {
    pub index: usize,
    pub xml: NodeId,
    pub id: String,
    pub element: String,
    pub parent: Option<usize>,
    /// Transform dependency; explicit @parent overrides containment for transforms only.
    pub transform_parent: Option<usize>,
    pub depth: usize,
    pub paint_position: usize,
    pub class: NodeClass,
    pub interval: Interval,
    pub span: Interval,
    /// Ancestor visibility window, before this node's transition handles.
    pub container_interval: Interval,
    /// True when no explicit end or finite intrinsic media duration defines the cut.
    pub end_inherited: bool,
    pub container_clock: Clock,
    pub local_start: f64,
    pub local_end: f64,
    pub source: Option<SourceMap>,
    pub layout: Option<Layout>,
    pub opacity: f64,
    pub visible: bool,
    pub animations: Vec<Animation>,
    pub unsupported: Vec<String>,
    /// Explicit attributes plus the XSD defaults; unknown attributes remain available for reporting.
    pub attributes: BTreeMap<String, String>,
}

/// A resolved transition, including handles beyond its participants' nominal intervals.
#[derive(Clone, Debug)]
pub struct Transition {
    pub node: Option<usize>,
    pub from: Option<usize>,
    pub to: Option<usize>,
    pub cut: f64,
    pub window: Interval,
    pub kind: String,
    pub curve: String,
}

/// One externally renderable subtree root, emitted once even when descendants are unsupported.
#[derive(Clone, Debug)]
pub struct Unmappable {
    pub node: usize,
    pub interval: Interval,
    pub paint_position: usize,
}

/// A structured observation for the import report.
#[derive(Clone, Debug)]
pub struct Note {
    pub node: Option<usize>,
    pub code: &'static str,
    pub message: String,
}

/// Complete, bounded semantic result, independent of document lifetimes.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub project: ProjectSettings,
    pub nodes: Vec<ResolvedNode>,
    pub transitions: Vec<Transition>,
    pub unmappable: Vec<Unmappable>,
    pub notes: Vec<Note>,
    positions: Vec<usize>,
}
impl Resolved {
    /// Gets a stable node index, independent of its position in paint order.
    pub fn node(&self, index: usize) -> Option<&ResolvedNode> {
        self.nodes.get(*self.positions.get(index)?)
    }
    /// Converts a key time on its declared timeline into composition seconds.
    pub fn key_time(&self, node: usize, time_base: TimeBase, t: f64) -> Option<f64> {
        key_time(self.node(node)?, time_base, t).ok()
    }
}

/// Resolves a validated document. Resource limits and unsafe numeric conversions always fail.
pub fn resolve(doc: &SceneDoc<'_>, limits: &ResolveLimits) -> Result<Resolved> {
    Resolver::new(doc, limits)?.run()
}

#[cfg(test)]
#[path = "resolve/tests.rs"]
mod tests;

const IDENTITY: Affine = Affine { a: 1., b: 0., c: 0., d: 1., e: 0., f: 0. };
const CLOCK: Clock = Clock { offset: 0., scale: 1. };

fn finite(v: f64) -> Result<f64> {
    if v.is_finite() { Ok(v) } else { Err(err("non-finite semantic computation")) }
}
fn time(v: f64) -> Result<f64> {
    finite(v)?;
    // Check representability without quantizing: the importer converts resolved seconds once.
    if v.abs() >= filmcraft_time::Tick::MAX.0 as f64 / filmcraft_time::TICKS_PER_SECOND as f64 {
        return Err(err("resolved seconds exceed Tick range"));
    }
    Ok(v)
}
fn number(n: Node<'_, '_>, name: &str, default: f64) -> Result<f64> {
    n.attribute(name).map_or(Ok(default), |s| values::double(s).map_err(|e| err(format!("{}@{name}: {e}", n.tag_name().name()))))
}
fn boolean(n: Node<'_, '_>, name: &str, default: bool) -> Result<bool> {
    n.attribute(name).map_or(Ok(default), |s| values::boolean(s).map_err(err))
}
fn required<'a>(n: Node<'a, '_>, name: &str) -> Result<&'a str> {
    n.attribute(name).ok_or_else(|| err(format!("{} requires @{name}", n.tag_name().name())))
}
fn positive(v: f64, name: &str) -> Result<f64> {
    finite(v)?;
    if v <= 0. { Err(err(format!("{name} must be positive"))) } else { Ok(v) }
}
fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}
fn mapped(clock: Clock, t: f64) -> Result<f64> {
    time(clock.offset + finite(clock.scale * t)?)
}
fn inverse(clock: Clock, t: f64) -> Result<f64> {
    positive(clock.scale, "clock scale")?;
    time(finite(t - clock.offset)? / clock.scale)
}
fn key_time(n: &ResolvedNode, base: TimeBase, t: f64) -> Result<f64> {
    finite(t)?;
    let local = match base {
        TimeBase::Composition => t,
        TimeBase::Local => time(n.local_start + t)?,
        TimeBase::Normalized => time(n.local_start + finite(t * finite(n.local_end - n.local_start)?)?)?,
    };
    mapped(n.container_clock, local)
}
fn multiply(p: Affine, q: Affine) -> Result<Affine> {
    Ok(Affine {
        a: finite(p.a * q.a + p.c * q.b)?,
        b: finite(p.b * q.a + p.d * q.b)?,
        c: finite(p.a * q.c + p.c * q.d)?,
        d: finite(p.b * q.c + p.d * q.d)?,
        e: finite(p.a * q.e + p.c * q.f + p.e)?,
        f: finite(p.b * q.e + p.d * q.f + p.f)?,
    })
}
fn matrix(t: Transform) -> Result<Affine> {
    let (s, c) = finite(t.rotation.to_radians())?.sin_cos();
    let kx = finite(finite(t.skew_x.to_radians())?.tan())?;
    let ky = finite(finite(t.skew_y.to_radians())?.tan())?;
    let linear = multiply(
        Affine { a: c, b: s, c: -s, d: c, ..IDENTITY },
        multiply(Affine { c: kx, b: ky, ..IDENTITY }, Affine { a: t.scale_x, d: t.scale_y, ..IDENTITY })?,
    )?;
    multiply(Affine { e: t.x, f: t.y, ..IDENTITY }, multiply(linear, Affine { e: -t.anchor_x, f: -t.anchor_y, ..IDENTITY })?)
}
fn is_node(n: Node<'_, '_>) -> bool {
    n.is_element()
        && matches!(
            n.tag_name().name(),
            "group"
                | "sequence"
                | "layer"
                | "shape"
                | "connector"
                | "object3D"
                | "camera"
                | "particleEmitter"
                | "particles3D"
                | "ocean"
                | "blackHole"
                | "accretionDisk"
                | "flock"
                | "fluid"
                | "slime"
                | "erosion"
                | "instance"
                | "include"
                | "repeat"
                | "adjustment"
                | "transition"
                | "skeleton"
                | "compute"
                | "iterate"
                | "program"
                | "viewport3D"
        )
}

fn stacking_z(element: &str, value: &str) -> Result<i64> {
    let declaration = validate::complex_type(&format!("{element}Type")).and_then(|ty| ty.attr("z"));
    if declaration.is_some_and(|attr| validate::root_builtin(attr.ty) == Some(validate::Builtin::Double)) {
        let z = values::double(value).map_err(err)?;
        // sr-eval exempts only these four; blackHole also uses its geometric z for stacking.
        if matches!(element, "object3D" | "particles3D" | "ocean" | "camera") {
            Ok(0)
        } else {
            let z = z.trunc().clamp(f64::from(i32::MIN), f64::from(i32::MAX));
            // Convert only a finite, bounded, nonnegative magnitude; retain authored text.
            let magnitude = z.abs() as i64;
            Ok(if z < 0.0 { -magnitude } else { magnitude })
        }
    } else {
        values::integer_in(value, i64::from(i32::MIN), i64::from(i32::MAX)).map_err(err)
    }
}

#[derive(Clone, Copy)]
struct Context {
    parent: Option<usize>,
    depth: usize,
    clock: Clock,
    window: Interval,
    shift: f64,
    width: f64,
    height: f64,
    opacity: f64,
    visible: bool,
    world: Option<Affine>,
    unsupported_root: bool,
}
struct Resolver<'a, 'i> {
    doc: &'a SceneDoc<'i>,
    limits: ResolveLimits,
    ids: HashMap<&'a str, Node<'a, 'i>>,
    out: Resolved,
    node_ids: HashMap<NodeId, usize>,
    segments: usize,
    keys: usize,
    children: HashMap<Option<usize>, Vec<usize>>,
}
impl<'a, 'i> Resolver<'a, 'i> {
    fn new(doc: &'a SceneDoc<'i>, limits: &ResolveLimits) -> Result<Self> {
        let mut ids = HashMap::new();
        let mut count = 0usize;
        for n in doc.doc.descendants().filter(Node::is_element) {
            count = count.checked_add(1).ok_or_else(|| err("node count overflow"))?;
            if count > limits.max_nodes.min(1_000_000) {
                return Err(err("semantic node limit exceeded"));
            }
            if let Some(id) = n.attribute("id")
                && ids.insert(id, n).is_some()
            {
                return Err(err(format!("duplicate id {id}")));
            }
        }
        let p = child(doc.root(), "project").ok_or_else(|| err("missing project"))?;
        let dimension = |name| -> Result<u32> {
            let v = values::integer_in(required(p, name)?, 1, i64::from(u32::MAX)).map_err(err)?;
            u32::try_from(v).map_err(|_| err("project dimension overflow"))
        };
        let project = ProjectSettings {
            width: dimension("width")?,
            height: dimension("height")?,
            fps: values::fps(required(p, "fps")?).map_err(err)?,
            duration: time(values::duration(required(p, "duration")?).map_err(err)?)?,
            background: values::paint(p.attribute("background").unwrap_or("#000000FF")).map_err(err)?,
        };
        Ok(Self {
            doc,
            limits: *limits,
            ids,
            out: Resolved { project, nodes: Vec::new(), transitions: Vec::new(), unmappable: Vec::new(), notes: Vec::new(), positions: Vec::new() },
            node_ids: HashMap::new(),
            segments: 0,
            keys: 0,
            children: HashMap::new(),
        })
    }
    fn note(&mut self, node: Option<usize>, code: &'static str, message: impl Into<String>) -> Result<()> {
        if self.out.notes.len() >= self.limits.max_notes.min(1_000_000) {
            return Err(err("semantic note limit exceeded"));
        }
        self.out.notes.push(Note { node, code, message: message.into() });
        Ok(())
    }
    fn run(mut self) -> Result<Resolved> {
        self.check_cycles()?;
        self.background()?;
        let root = child(self.doc.root(), "composition").ok_or_else(|| err("missing composition"))?;
        let p = &self.out.project;
        let ctx = Context {
            parent: None,
            depth: 0,
            clock: CLOCK,
            window: Interval { start: 0., end: p.duration },
            shift: 0.,
            width: f64::from(p.width),
            height: f64::from(p.height),
            opacity: 1.,
            visible: true,
            world: Some(IDENTITY),
            unsupported_root: false,
        };
        self.walk_children(root, ctx)?;
        self.flatten_parents()?;
        self.transitions()?;
        self.propagate_handles()?;
        self.media()?;
        self.finish_paint()?;
        Ok(self.out)
    }
    fn background(&mut self) -> Result<()> {
        let mut seen = HashSet::new();
        while let Paint::Var(name) = &self.out.project.background {
            if !seen.insert(name.clone()) || seen.len() > 8 {
                return Err(err("background token cycle or depth over 8"));
            }
            let token = child(self.doc.root(), "styles").and_then(|s| s.children().find(|t| t.has_tag_name("token") && t.attribute("name") == Some(name)));
            let token = token.ok_or_else(|| err(format!("unknown background token {name}")))?;
            self.out.project.background = values::paint(required(token, "value")?).map_err(err)?;
        }
        if matches!(self.out.project.background, Paint::Ref(_)) {
            self.note(None, "BACKGROUND_PAINT", "Background references a non-solid paint; native background mapping requires a solid colour")?;
        }
        Ok(())
    }
    fn marker(&self, name: &str) -> Result<f64> {
        if let Some(n) = self.ids.get(name).filter(|n| n.has_tag_name("marker")) {
            return time(number(*n, "time", 0.)?);
        }
        let (kind, index) = name.split_once('.').ok_or_else(|| err(format!("unknown marker {name}")))?;
        if !matches!(kind, "beat" | "bar") {
            return Err(err(format!("unknown marker {name}")));
        }
        let index = values::integer_in(index, 0, 1_000_000_000).map_err(err)?;
        let grid = child(self.doc.root(), "markers").and_then(|n| child(n, "beatGrid")).ok_or_else(|| err("marker needs a beatGrid"))?;
        let bpm = positive(number(grid, "bpm", 0.)?, "bpm")?;
        let beats = if kind == "bar" { positive(number(grid, "beatsPerBar", 4.)?, "beatsPerBar")? } else { 1. };
        time(number(grid, "offset", 0.)? + finite(finite(index as f64 * beats)? * finite(60. / bpm)?)?)
    }
    fn marker_offset(&self, n: Node<'_, '_>, attr: &str) -> Result<f64> {
        n.attribute(attr).map_or(Ok(0.), |s| self.marker(s))
    }
    fn check_cycles(&self) -> Result<()> {
        // Transform dependencies point upward; matte/symbol dependencies point into content.
        // Keep these graphs separate so ordinary XML parents do not create artificial cycles.
        for mode in 0..3 {
            let mut done = HashSet::new();
            let mut active = HashSet::new();
            for &root in self.ids.values() {
                if done.contains(&root.id()) {
                    continue;
                }
                let mut stack = vec![(root, false)];
                while let Some((n, exit)) = stack.pop() {
                    if exit {
                        active.remove(&n.id());
                        done.insert(n.id());
                        continue;
                    }
                    if done.contains(&n.id()) {
                        continue;
                    }
                    if !active.insert(n.id()) {
                        return Err(err("reference cycle in parent, matte or group dependencies"));
                    }
                    if active.len() > self.limits.max_depth.min(128).saturating_add(1) {
                        return Err(err("reference depth limit exceeded"));
                    }
                    stack.push((n, true));
                    if mode == 0 {
                        if let Some(id) = n.attribute("parent") {
                            let target = self.ids.get(id).ok_or_else(|| err(format!("unknown parent {id}")))?;
                            stack.push((*target, false));
                        } else if let Some(p) = n.parent_element().filter(|p| is_node(*p)) {
                            stack.push((p, false));
                        }
                    } else {
                        for c in n.children().filter(Node::is_element) {
                            stack.push((c, false));
                        }
                        let attr = if mode == 1 { "matte" } else { "symbol" };
                        if let Some(id) = n.attribute(attr) {
                            let target = self.ids.get(id).ok_or_else(|| err(format!("unknown {attr} {id}")))?;
                            stack.push((*target, false));
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn asset(&self, n: Node<'_, '_>) -> Option<Node<'a, 'i>> {
        n.attribute("asset").and_then(|s| self.ids.get(s)).copied()
    }
    fn class(&self, n: Node<'_, '_>) -> Result<NodeClass> {
        if boolean(n, "threeD", false)? {
            return Ok(NodeClass::Unmappable);
        }
        Ok(match n.tag_name().name() {
            "layer" => match self.asset(n).map(|n| n.tag_name().name()) {
                Some("video") => NodeClass::Media(MediaKind::Video),
                Some("image") => NodeClass::Media(MediaKind::Image),
                Some("audio") => NodeClass::Media(MediaKind::Audio),
                Some("imageSequence") => NodeClass::Media(MediaKind::ImageSequence),
                Some("text") => NodeClass::Text,
                _ => NodeClass::Unmappable,
            },
            "shape" => match n.attribute("shape") {
                Some(s @ ("rect" | "rounded-rect" | "ellipse" | "polygon" | "star" | "line" | "path")) => NodeClass::Shape(s.into()),
                _ => NodeClass::Unmappable,
            },
            "group" => NodeClass::Group {
                isolated: boolean(n, "isolate", false)?
                    || boolean(n, "clip", false)?
                    || n.attribute("effects").is_some()
                    || n.attribute("matte").is_some()
                    || n.attribute("blend").is_some_and(|b| b != "normal")
                    || child(n, "mask").is_some(),
                retimed: number(n, "timeOffset", 0.)? != 0. || number(n, "timeScale", 1.)? != 1.,
            },
            "sequence" => NodeClass::Sequence,
            "transition" => NodeClass::Transition,
            _ => NodeClass::Unmappable,
        })
    }
    fn attributes(&self, n: Node<'_, '_>, id: &str) -> BTreeMap<String, String> {
        let ty = format!("{}Type", n.tag_name().name());
        let mut attrs = BTreeMap::new();
        if let Some(ty) = validate::complex_type(&ty) {
            for a in ty.attrs {
                if let Some(v) = a.default {
                    attrs.insert(a.name.into(), v.into());
                }
            }
        }
        for a in n.attributes() {
            attrs.insert(a.name().into(), a.value().into());
        }
        attrs.insert("id".into(), id.into());
        attrs
    }
    fn walk_children(&mut self, container: Node<'a, 'i>, ctx: Context) -> Result<()> {
        let sequence = container.has_tag_name("sequence");
        let mut cursor = if sequence { self.out.nodes.get(ctx.parent.unwrap_or(usize::MAX)).map_or(0., |n| n.local_start) } else { 0. };
        for n in container.children().filter(|n| is_node(*n)) {
            let mut own_ctx = ctx;
            if sequence && !n.has_tag_name("transition") && n.attribute("startMarker").is_none() {
                own_ctx.shift = cursor;
            }
            let index = self.walk(n, own_ctx)?;
            if sequence && !n.has_tag_name("transition") {
                let resolved = self.out.nodes.get(index).ok_or_else(|| err("missing sequence child"))?;
                cursor = time(resolved.local_end + number(container, "timeGap", 0.)?)?;
            }
        }
        Ok(())
    }
    fn walk(&mut self, n: Node<'a, 'i>, ctx: Context) -> Result<usize> {
        if ctx.depth > self.limits.max_depth.min(128) {
            return Err(err("composition depth limit exceeded"));
        }
        if self.out.nodes.len() >= self.limits.max_nodes.min(1_000_000) {
            return Err(err("composition node limit exceeded"));
        }
        let index = self.out.nodes.len();
        let id = n.attribute("id").map(str::to_owned).unwrap_or_else(|| format!("@scene-node-{index}"));
        let class = self.class(n)?;
        let start = time(number(n, "start", 0.)? + self.marker_offset(n, "startMarker")? + ctx.shift)?;
        let remap = child(n, "timeRemap");
        let speed = number(n, "speed", 1.)?;
        let stretch = positive(number(n, "timeStretch", 1.)?, "timeStretch")?;
        let rate = if remap.is_some() { 1. } else { finite(speed / stretch)? };
        if remap.is_none() && speed != 0. && rate == 0. {
            return Err(err("playback rate underflow"));
        }
        let plays = if remap.is_some() {
            1
        } else {
            values::unsigned_long(n.attribute("loop").unwrap_or("0")).map_err(err)?.checked_add(1).ok_or_else(|| err("loop count overflow"))?
        };
        if remap.is_none() && matches!(class, NodeClass::Media(_)) && plays > self.limits.max_segments.min(1_000_000) as u64 {
            return Err(err("source segment expansion exceeds limit"));
        }
        let source_span = if matches!(class, NodeClass::Media(_)) { self.source_span(n)? } else { None };
        let inherited_end = inverse(ctx.clock, ctx.window.end)?;
        let end_inherited = n.attribute("end").is_none()
            && n.attribute("endMarker").is_none()
            && (source_span.is_none() || remap.is_some() || n.attribute("freezeAt").is_some() || rate == 0.);
        let end = if n.attribute("end").is_some() || n.attribute("endMarker").is_some() {
            time(number(n, "end", 0.)? + self.marker_offset(n, "endMarker")? + ctx.shift)?
        } else if let Some((clip_in, clip_out)) = source_span.filter(|_| remap.is_none() && n.attribute("freezeAt").is_none() && rate != 0.) {
            time(start + finite(finite((clip_out - clip_in) * plays as f64)? / rate.abs())?)?
        } else {
            inherited_end
        };
        let raw_start = mapped(ctx.clock, start)?;
        let raw_end = mapped(ctx.clock, end)?;
        let interval = Interval { start: raw_start.max(ctx.window.start), end: raw_end.min(ctx.window.end) };
        let opacity = finite(ctx.opacity * values::unit(n.attribute("opacity").unwrap_or("1")).map_err(err)?)?;
        let visible = ctx.visible && boolean(n, "visible", true)?;
        let (animations, unsupported) = self.behaviours(n, index, start, end, ctx.clock)?;
        let layout = if matches!(class, NodeClass::Media(MediaKind::Audio) | NodeClass::Transition) {
            None
        } else {
            Some(self.layout(n, ctx, &animations, &unsupported)?)
        };
        for name in &unsupported {
            self.note(Some(index), "UNSUPPORTED_FEATURE", format!("Unsupported feature {name}"))?;
        }
        let resolved = ResolvedNode {
            index,
            xml: n.id(),
            id: id.clone(),
            element: n.tag_name().name().into(),
            parent: ctx.parent,
            transform_parent: ctx.parent,
            depth: ctx.depth,
            paint_position: 0,
            class,
            interval,
            span: interval,
            container_interval: ctx.window,
            end_inherited,
            container_clock: ctx.clock,
            local_start: start,
            local_end: end,
            source: None,
            layout,
            opacity,
            visible,
            animations,
            unsupported,
            attributes: self.attributes(n, &id),
        };
        let is_unmapped = resolved.class == NodeClass::Unmappable;
        if is_unmapped && !ctx.unsupported_root {
            self.out.unmappable.push(Unmappable { node: index, interval, paint_position: 0 });
            self.note(Some(index), "UNMAPPABLE_SUBTREE", format!("Subtree {id} ({}) requires external rendering", n.tag_name().name()))?;
        }
        let child_size = resolved.layout.as_ref().map_or((ctx.width, ctx.height), |l| (l.width, l.height));
        let child_world = resolved.layout.as_ref().and_then(|l| l.world_matrix);
        self.out.nodes.push(resolved);
        self.node_ids.insert(n.id(), index);
        self.children.entry(ctx.parent).or_default().push(index);
        let scale = positive(number(n, "timeScale", 1.)?, "timeScale")?;
        let offset = number(n, "timeOffset", 0.)?;
        // sr-eval/eval.rs::clock_time: child = origin + (parent-origin-offset)*scale.
        let clock = if matches!(n.tag_name().name(), "group" | "sequence") {
            let scale_inv = positive(finite(ctx.clock.scale / scale)?, "inverse clock scale")?;
            Clock { scale: scale_inv, offset: time(ctx.clock.offset + finite(ctx.clock.scale * finite(start + offset - finite(start / scale)?)?)?)? }
        } else {
            ctx.clock
        };
        let child_ctx = Context {
            parent: Some(index),
            depth: ctx.depth.saturating_add(1),
            clock,
            window: interval,
            shift: ctx.shift,
            width: child_size.0,
            height: child_size.1,
            opacity,
            visible,
            world: child_world,
            unsupported_root: ctx.unsupported_root || is_unmapped,
        };
        self.walk_children(n, child_ctx)?;
        Ok(index)
    }
    fn flatten_parents(&mut self) -> Result<()> {
        for i in 0..self.out.nodes.len() {
            let node = self.out.nodes.get(i).ok_or_else(|| err("transform index"))?;
            let parent = match node.attributes.get("parent") {
                Some(id) => Some(
                    *self
                        .ids
                        .get(id.as_str())
                        .and_then(|n| self.node_ids.get(&n.id()))
                        .ok_or_else(|| err(format!("transform parent {id} is not a composition node")))?,
                ),
                None => node.parent,
            };
            self.out.nodes.get_mut(i).ok_or_else(|| err("transform index"))?.transform_parent = parent;
        }
        let mut resolved = vec![false; self.out.nodes.len()];
        for root in 0..self.out.nodes.len() {
            let mut path = Vec::new();
            let mut cursor = Some(root);
            while let Some(i) = cursor {
                if resolved.get(i).copied().unwrap_or(false) {
                    break;
                }
                if path.len() > self.limits.max_depth.min(128) {
                    return Err(err("transform parent depth or cycle exceeds limit"));
                }
                let node = self.out.nodes.get(i).ok_or_else(|| err("transform parent index"))?;
                path.push(i);
                cursor = node.transform_parent;
            }
            for i in path.into_iter().rev() {
                let node = self.out.nodes.get(i).ok_or_else(|| err("transform index"))?;
                let base = match node.transform_parent {
                    Some(parent) => self.out.nodes.get(parent).and_then(|p| p.layout.as_ref()).and_then(|l| l.world_matrix),
                    None => Some(IDENTITY),
                };
                if let Some(layout) = &node.layout {
                    let product = base.map(|p| multiply(p, layout.local_matrix)).transpose()?;
                    let static_transform = layout.transform_static;
                    let layout = self.out.nodes.get_mut(i).and_then(|n| n.layout.as_mut()).ok_or_else(|| err("missing transform layout"))?;
                    layout.world_matrix = if static_transform { product } else { None };
                    layout.parent_transform_unresolved = base.is_none();
                    if base.is_none() {
                        self.note(Some(i), "PARENT_TRANSFORM", "An animated or unsupported parent transform cannot be flattened statically")?;
                    }
                }
                *resolved.get_mut(i).ok_or_else(|| err("transform result index"))? = true;
            }
        }
        Ok(())
    }
    fn propagate_handles(&mut self) -> Result<()> {
        let mut handles: HashMap<usize, Interval> = HashMap::new();
        for t in &self.out.transitions {
            for i in [t.from, t.to].into_iter().flatten() {
                handles
                    .entry(i)
                    .and_modify(|w| {
                        w.start = w.start.min(t.window.start);
                        w.end = w.end.max(t.window.end);
                    })
                    .or_insert(t.window);
            }
        }
        // Stable indices are document preorder, so each containment parent's span is ready first.
        for i in 0..self.out.nodes.len() {
            let n = self.out.nodes.get(i).ok_or_else(|| err("handle index"))?;
            let container = match n.parent {
                Some(p) => self.out.nodes.get(p).ok_or_else(|| err("handle parent index"))?.span,
                None => Interval { start: 0., end: self.out.project.duration },
            };
            let mut span = if n.class == NodeClass::Transition {
                n.interval
            } else {
                Interval {
                    start: mapped(n.container_clock, n.local_start)?,
                    end: if n.end_inherited { container.end } else { mapped(n.container_clock, n.local_end)? },
                }
            };
            if let Some(h) = handles.get(&i) {
                span.start = span.start.min(h.start);
                span.end = span.end.max(h.end);
            }
            span.start = span.start.max(container.start);
            span.end = span.end.min(container.end);
            let n = self.out.nodes.get_mut(i).ok_or_else(|| err("handle index"))?;
            n.span = span;
            n.container_interval = container;
        }
        Ok(())
    }
    fn media(&mut self) -> Result<()> {
        for i in 0..self.out.nodes.len() {
            let node = self.out.nodes.get(i).filter(|n| matches!(n.class, NodeClass::Media(_))).cloned();
            let Some(node) = node else { continue };
            let xml = self.doc.doc.get_node(node.xml).ok_or_else(|| err("missing media XML"))?;
            let span = self.source_span(xml)?;
            let plays = if child(xml, "timeRemap").is_some() {
                1
            } else {
                values::unsigned_long(xml.attribute("loop").unwrap_or("0")).map_err(err)?.checked_add(1).ok_or_else(|| err("loop count overflow"))?
            };
            let rate = if child(xml, "timeRemap").is_some() {
                1.
            } else {
                finite(number(xml, "speed", 1.)? / positive(number(xml, "timeStretch", 1.)?, "timeStretch")?)?
            };
            let source = self.source(xml, &node, span, plays, rate)?;
            self.out.nodes.get_mut(i).ok_or_else(|| err("media node index"))?.source = Some(source);
        }
        Ok(())
    }
    fn source_span(&self, n: Node<'_, '_>) -> Result<Option<(f64, f64)>> {
        let clip_in = time(number(n, "clipIn", 0.)?)?;
        let end = if n.attribute("clipOut").is_some() {
            Some(time(number(n, "clipOut", 0.)?)?)
        } else if let Some(a) = self.asset(n) {
            if a.attribute("duration").is_some() {
                Some(time(number(a, "duration", 0.)?)?)
            } else if a.has_tag_name("imageSequence") {
                let first = values::integer_in(required(a, "first")?, i64::MIN, i64::MAX).map_err(err)?;
                let last = values::integer_in(required(a, "last")?, i64::MIN, i64::MAX).map_err(err)?;
                let step = values::integer_in(a.attribute("step").unwrap_or("1"), 1, i64::MAX).map_err(err)?;
                let count = last
                    .checked_sub(first)
                    .and_then(|d| d.checked_div(step))
                    .and_then(|d| d.checked_add(1))
                    .filter(|d| *d > 0)
                    .ok_or_else(|| err("image sequence frame count overflow or invalid range"))?;
                let fps = values::fps(required(a, "fps")?).map_err(err)?;
                Some(time(count as f64 * fps.den as f64 / fps.num as f64)?)
            } else {
                None
            }
        } else {
            None
        };
        match end {
            Some(end) if end >= clip_in => Ok(Some((clip_in, end))),
            Some(_) => Err(err("clipOut precedes clipIn")),
            None => Ok(None),
        }
    }
    fn add_segment(&mut self, out: &mut Vec<SourceSegment>, segment: SourceSegment) -> Result<()> {
        for t in [segment.comp_start, segment.comp_end, segment.src_start] {
            time(t)?;
        }
        finite(segment.rate)?;
        time(segment.src_start + finite((segment.comp_end - segment.comp_start) * segment.rate)?)?;
        if segment.comp_end <= segment.comp_start {
            return Ok(());
        }
        if self.segments >= self.limits.max_segments.min(1_000_000) {
            return Err(err("total source segment limit exceeded"));
        }
        self.segments = self.segments.checked_add(1).ok_or_else(|| err("source segment count overflow"))?;
        out.push(segment);
        Ok(())
    }
    fn key_budget(&mut self) -> Result<()> {
        if self.keys >= self.limits.max_keys.min(1_000_000) {
            return Err(err("animation key limit exceeded"));
        }
        self.keys = self.keys.checked_add(1).ok_or_else(|| err("key count overflow"))?;
        Ok(())
    }
    fn source(&mut self, n: Node<'a, 'i>, node: &ResolvedNode, span: Option<(f64, f64)>, plays: u64, rate: f64) -> Result<SourceMap> {
        let freeze = n.attribute("freezeAt").map(|s| values::double(s).map_err(err).and_then(time)).transpose()?;
        if let Some(remap) = child(n, "timeRemap") {
            let mut keys = Vec::new();
            let mut previous = None;
            for k in remap.children().filter(|n| n.has_tag_name("key")) {
                self.key_budget()?;
                let local = time(number(k, "time", 0.)? + self.marker_offset(k, "marker")?)?;
                let comp_time = key_time(node, TimeBase::Local, local)?;
                if previous.is_some_and(|p| comp_time < p) {
                    return Err(err("remap keys decrease in time"));
                }
                previous = Some(comp_time);
                keys.push(RemapKey {
                    xml: k.id(),
                    comp_time,
                    source_time: time(values::double(required(k, "value")?).map_err(err)?)?,
                    interpolation: k.attribute("interpolation").or_else(|| remap.attribute("defaultInterpolation")).unwrap_or("linear").into(),
                });
            }
            if keys.is_empty() {
                return Err(err("timeRemap needs keys"));
            }
            if freeze.is_some() {
                self.note(Some(node.index), "FROZEN_REMAP", "freezeAt selects a fixed layer-local time on the remap curve")?;
            }
            return Ok(SourceMap::Remap { keys, freeze_local: freeze });
        }
        let (clip_in, clip_out) = if let Some(span) = span {
            span
        } else {
            let clip_in = time(number(n, "clipIn", 0.)?)?;
            let inferred = time(finite((node.local_end - node.local_start).max(0.) * if rate == 0. { 1. } else { rate.abs() })? / plays as f64)?;
            self.note(Some(node.index), "MEDIA_DURATION_INFERRED", "Media duration is unknown; source span inferred from the node interval and playback rate")?;
            (clip_in, time(clip_in + inferred)?)
        };
        if n.attribute("clipOut").is_some() && self.asset(n).is_none_or(|a| a.attribute("duration").is_none() && !a.has_tag_name("imageSequence")) {
            self.note(Some(node.index), "MEDIA_DURATION_INFERRED", "Media duration is unknown; clipOut defines the source span")?;
        }
        let length = time(clip_out - clip_in)?;
        let reverse = boolean(n, "reverse", false)? != (rate < 0.);
        let source_at = |local: f64| -> Result<f64> {
            let mut offset = time(local * rate.abs())?;
            if plays > 1 && length > 0. {
                let total = time(length * plays as f64)?;
                offset = if offset >= total {
                    length
                } else if offset >= 0. {
                    offset % length
                } else {
                    offset
                };
            }
            time(if reverse { clip_out - offset } else { clip_in + offset })
        };
        let mut segments = Vec::new();
        if freeze.is_some() || rate == 0. || length == 0. {
            self.add_segment(
                &mut segments,
                SourceSegment { comp_start: node.span.start, comp_end: node.span.end, src_start: source_at(freeze.unwrap_or(0.))?, rate: 0. },
            )?;
            return Ok(SourceMap::Segments(segments));
        }
        let play_duration = positive(time(length / rate.abs())?, "play duration")?;
        let comp_rate = finite(if reverse { -rate.abs() } else { rate.abs() } / positive(node.container_clock.scale, "clock scale")?)?;
        let raw_start = mapped(node.container_clock, node.local_start)?;
        if node.span.start < raw_start {
            self.add_segment(
                &mut segments,
                SourceSegment {
                    comp_start: node.span.start,
                    comp_end: raw_start.min(node.span.end),
                    src_start: source_at(time(inverse(node.container_clock, node.span.start)? - node.local_start)?)?,
                    rate: comp_rate,
                },
            )?;
        }
        let mut cursor = node.local_start;
        for _ in 0..plays {
            let next = time(cursor + play_duration)?;
            if next <= cursor {
                return Err(err("play duration is below floating-point time resolution"));
            }
            let comp_start = mapped(node.container_clock, cursor)?;
            let comp_end = mapped(node.container_clock, next)?;
            let start = comp_start.max(node.span.start);
            let end = comp_end.min(node.span.end);
            let src = time(if reverse { clip_out } else { clip_in } + finite((start - comp_start) * comp_rate)?)?;
            self.add_segment(&mut segments, SourceSegment { comp_start: start, comp_end: end, src_start: src, rate: comp_rate })?;
            cursor = next;
            if comp_end >= node.span.end {
                break;
            }
        }
        let comp_end = mapped(node.container_clock, cursor)?;
        if comp_end < node.span.end {
            let start = comp_end.max(node.span.start);
            self.add_segment(
                &mut segments,
                SourceSegment {
                    comp_start: start,
                    comp_end: node.span.end,
                    src_start: source_at(time(inverse(node.container_clock, start)? - node.local_start)?)?,
                    rate: if plays > 1 { 0. } else { comp_rate },
                },
            )?;
            self.note(
                Some(node.index),
                "SOURCE_END_EXTENDED",
                if plays > 1 {
                    "Node interval exceeds its repeated source; terminal source frame is held"
                } else {
                    "Explicit end or handles exceed clipOut; unlooped source clock continues (decoder may hold at media end)"
                },
            )?;
        }
        Ok(SourceMap::Segments(segments))
    }
    fn behaviours(&mut self, n: Node<'a, 'i>, index: usize, start: f64, end: f64, clock: Clock) -> Result<(Vec<Animation>, Vec<String>)> {
        let mut animations = Vec::new();
        let mut unsupported = Vec::new();
        let mut pending: Vec<_> = n.children().filter(|c| c.is_element() && !is_node(*c)).collect();
        while let Some(c) = pending.pop() {
            let name = c.tag_name().name();
            if name == "animate" {
                let base = match c.attribute("timeBase").unwrap_or("composition") {
                    "composition" => TimeBase::Composition,
                    "local" => TimeBase::Local,
                    "normalized" => TimeBase::Normalized,
                    _ => return Err(err("unknown animation time base")),
                };
                let mut keys = Vec::new();
                for k in c.children().filter(|c| c.has_tag_name("key")) {
                    self.key_budget()?;
                    let t = time(number(k, "time", 0.)? + self.marker_offset(k, "marker")?)?;
                    let local = match base {
                        TimeBase::Composition => t,
                        TimeBase::Local => time(start + t)?,
                        TimeBase::Normalized => time(start + finite(t * finite(end - start)?)?)?,
                    };
                    keys.push((k.id(), mapped(clock, local)?));
                    if boolean(k, "roving", false)? {
                        unsupported.push("roving".into());
                    }
                }
                animations.push(Animation {
                    xml: c.id(),
                    target: c.parent_element().map_or(n.id(), |p| p.id()),
                    property: required(c, "property")?.into(),
                    time_base: base,
                    keys,
                    default_interpolation: c.attribute("defaultInterpolation").unwrap_or("linear").into(),
                });
                if boolean(c, "additive", false)? {
                    unsupported.push("additive".into());
                }
                for a in ["extrapolateBefore", "extrapolateAfter"] {
                    if c.attribute(a).is_some_and(|s| s != "hold") {
                        unsupported.push(a.into());
                    }
                }
            } else if name != "timeRemap" {
                unsupported.push(name.into());
                pending.extend(c.children().filter(|c| c.is_element() && !is_node(*c)));
            }
        }
        for attr in ["condition", "matte", "effects"] {
            if n.attribute(attr).is_some() {
                unsupported.push(attr.into());
            }
        }
        if n.attribute("layout").is_some_and(|s| s != "none") {
            unsupported.push("layout".into());
        }
        if n.attribute("alignTo") == Some("safe-area") {
            unsupported.push("safe-area".into());
        }
        unsupported.sort();
        unsupported.dedup();
        if matches!(n.tag_name().name(), "group" | "sequence") && animations.iter().any(|a| a.property == "opacity") {
            self.note(Some(index), "ANIMATED_GROUP_OPACITY", "Group opacity is animated; its static product alone does not represent descendant opacity")?;
        }
        Ok((animations, unsupported))
    }
    fn layout(&self, n: Node<'_, '_>, ctx: Context, animations: &[Animation], unsupported: &[String]) -> Result<Layout> {
        let fw = f64::from(self.out.project.width);
        let fh = f64::from(self.out.project.height);
        let length = |node: Node<'_, '_>, name: &str, parent: f64, default: f64| -> Result<f64> {
            match node.attribute(name) {
                Some(s) => finite(values::length(s).map_err(err)?.resolve(parent, fw, fh)),
                None => Ok(default),
            }
        };
        let natural = self.asset(n).and_then(|a| a.attribute("width").zip(a.attribute("height")));
        let natural = natural
            .map(|(w, h)| -> Result<_> {
                Ok((positive(values::double(w).map_err(err)?, "asset width")?, positive(values::double(h).map_err(err)?, "asset height")?))
            })
            .transpose()?;
        let fitted = n.attribute("fit").is_some_and(|s| s != "none");
        let (wattr, hattr) = if fitted { ("boxWidth", "boxHeight") } else { ("width", "height") };
        let has_box = n.attribute(wattr).is_some() && n.attribute(hattr).is_some();
        let (width, height) = if has_box {
            (length(n, wattr, ctx.width, ctx.width)?, length(n, hattr, ctx.height, ctx.height)?)
        } else {
            natural.unwrap_or((ctx.width, ctx.height))
        };
        positive(width, "layout width")?;
        positive(height, "layout height")?;
        let mut transform = Transform {
            x: length(n, "x", ctx.width, 0.)?,
            y: length(n, "y", ctx.height, 0.)?,
            // XSD explicitly bases relative anchors on the parent, even for a fitted layer.
            anchor_x: length(n, "anchorX", ctx.width, 0.)?,
            anchor_y: length(n, "anchorY", ctx.height, 0.)?,
            scale_x: number(n, "scaleX", 1.)?,
            scale_y: number(n, "scaleY", 1.)?,
            rotation: number(n, "rotation", 0.)?,
            skew_x: number(n, "skewX", 0.)?,
            skew_y: number(n, "skewY", 0.)?,
        };
        let frame = n.attribute("alignTo") == Some("frame");
        let (bw, bh) = if frame { (fw, fh) } else { (ctx.width, ctx.height) };
        let margin = length(n, "margin", ctx.width, 0.)?;
        let mut width = width;
        let mut height = height;
        if has_box || natural.is_some() {
            match n.attribute("alignX") {
                Some("left") => transform.x = margin,
                Some("center") => transform.x = finite((bw - width) / 2.)?,
                Some("right") => transform.x = finite(bw - width - margin)?,
                Some("stretch") => {
                    transform.x = margin;
                    width = positive(finite(bw - 2. * margin)?, "stretched width")?;
                }
                _ => {}
            }
            match n.attribute("alignY") {
                Some("top") => transform.y = margin,
                Some("middle") => transform.y = finite((bh - height) / 2.)?,
                Some("bottom") => transform.y = finite(bh - height - margin)?,
                Some("stretch") => {
                    transform.y = margin;
                    height = positive(finite(bh - 2. * margin)?, "stretched height")?;
                }
                _ => {}
            }
        }
        let local_matrix = matrix(transform)?;
        let animated = animations.iter().any(|a| {
            a.target == n.id()
                && matches!(
                    a.property.as_str(),
                    "x" | "y"
                        | "position"
                        | "anchor"
                        | "anchorX"
                        | "anchorY"
                        | "scale"
                        | "scaleX"
                        | "scaleY"
                        | "rotation"
                        | "skew"
                        | "skewX"
                        | "skewY"
                        | "width"
                        | "height"
                )
        });
        let unsupported_transform =
            unsupported.iter().any(|s| matches!(s.as_str(), "expression" | "link" | "transformConstraint" | "motionPath" | "layout" | "safe-area"))
                || boolean(n, "threeD", false)?;
        // Always compute the static product, even when flagged, to reject overflow instead of hiding it.
        let composed = ctx.world.map(|p| multiply(p, local_matrix)).transpose()?;
        Ok(Layout {
            width,
            height,
            size_known: has_box || natural.is_some() || matches!(n.tag_name().name(), "group" | "sequence"),
            transform,
            local_matrix,
            transform_static: !animated && !unsupported_transform,
            world_matrix: if animated || unsupported_transform { None } else { composed },
            parent_transform_unresolved: ctx.world.is_none(),
        })
    }
    fn transition(
        &mut self,
        node: Option<usize>,
        from: Option<usize>,
        to: Option<usize>,
        duration: f64,
        alignment: &str,
        kind: &str,
        curve: &str,
    ) -> Result<()> {
        if self.out.transitions.len() >= self.limits.max_nodes.min(1_000_000) {
            return Err(err("transition limit exceeded"));
        }
        let a = from.and_then(|i| self.out.nodes.get(i));
        let b = to.and_then(|i| self.out.nodes.get(i));
        if a.is_none() && b.is_none() {
            return Err(err("transition needs from or to"));
        }
        if let (Some(a), Some(b)) = (a, b)
            && a.parent != b.parent
        {
            return Err(err("transition endpoints must be siblings"));
        }

        let cut = match (a, b) {
            (Some(a), _) if !a.end_inherited => mapped(a.container_clock, a.local_end)?,
            (_, Some(b)) => mapped(b.container_clock, b.local_start)?,
            (Some(a), None) => mapped(a.container_clock, a.local_end)?,
            _ => return Err(err("transition has no cut")),
        };
        let scale = a.or(b).map_or(1., |n| n.container_clock.scale);
        let d = time(positive(duration, "transition duration")? * scale)?;
        let window = match alignment {
            "center" => Interval { start: time(cut - d / 2.)?, end: time(cut + d / 2.)? },
            "start" => Interval { start: cut, end: time(cut + d)? },
            "end" => Interval { start: time(cut - d)?, end: cut },
            _ => return Err(err("unknown transition alignment")),
        };
        for i in [from, to].into_iter().flatten() {
            let n = self.out.nodes.get_mut(i).ok_or_else(|| err("transition endpoint index"))?;
            n.span.start = n.span.start.min(window.start).max(n.container_interval.start);
            n.span.end = n.span.end.max(window.end).min(n.container_interval.end);
        }
        if let Some(i) = node {
            let n = self.out.nodes.get_mut(i).ok_or_else(|| err("transition node index"))?;
            n.interval = window;
            n.span = window;
        }
        self.out.transitions.push(Transition { node, from, to, cut, window, kind: kind.into(), curve: curve.into() });
        Ok(())
    }
    fn transitions(&mut self) -> Result<()> {
        let nodes: Vec<_> = self.out.nodes.iter().map(|n| (n.index, n.xml)).collect();
        let mut explicit = HashSet::new();
        for &(index, xml) in &nodes {
            let n = self.doc.doc.get_node(xml).ok_or_else(|| err("missing XML node"))?;
            if !n.has_tag_name("transition") {
                continue;
            }
            let endpoint = |name: &str| -> Result<Option<usize>> {
                n.attribute(name)
                    .map(|id| self.ids.get(id).and_then(|n| self.node_ids.get(&n.id())).copied().ok_or_else(|| err(format!("unknown transition {name}: {id}"))))
                    .transpose()
            };
            let from = endpoint("from")?;
            let to = endpoint("to")?;
            explicit.insert((from, to));
            self.transition(
                Some(index),
                from,
                to,
                number(n, "duration", 0.5)?,
                n.attribute("alignment").unwrap_or("center"),
                required(n, "type")?,
                n.attribute("curve").unwrap_or("ease-in-out"),
            )?;
        }
        for (index, xml) in nodes {
            let n = self.doc.doc.get_node(xml).ok_or_else(|| err("missing XML node"))?;
            if !n.has_tag_name("sequence") {
                continue;
            }
            let Some(kind) = n.attribute("transition") else { continue };
            let kids: Vec<_> = self
                .children
                .get(&Some(index))
                .into_iter()
                .flatten()
                .copied()
                .filter(|i| self.out.nodes.get(*i).is_some_and(|n| n.class != NodeClass::Transition))
                .collect();
            for pair in kids.windows(2) {
                let (Some(&a), Some(&b)) = (pair.first(), pair.get(1)) else { continue };
                if !explicit.contains(&(Some(a), Some(b))) {
                    self.transition(None, Some(a), Some(b), number(n, "transitionDuration", 0.5)?, "center", kind, "ease-in-out")?;
                }
            }
        }
        Ok(())
    }
    fn finish_paint(&mut self) -> Result<()> {
        for kids in self.children.values_mut() {
            let mut ordered = Vec::with_capacity(kids.len());
            for &i in kids.iter() {
                let n = self.out.nodes.get(i).ok_or_else(|| err("paint index"))?;
                let z = stacking_z(&n.element, n.attributes.get("z").map(String::as_str).unwrap_or("0"))?;
                ordered.push((z, i));
            }
            ordered.sort_by_key(|&(z, _)| z);
            *kids = ordered.into_iter().map(|(_, i)| i).collect();
        }
        let mut pending: Vec<_> = self.children.get(&None).into_iter().flatten().rev().copied().collect();
        let mut position = 0usize;
        while let Some(i) = pending.pop() {
            let n = self.out.nodes.get_mut(i).ok_or_else(|| err("paint index"))?;
            n.paint_position = position;
            position = position.checked_add(1).ok_or_else(|| err("paint position overflow"))?;
            pending.extend(self.children.get(&Some(i)).into_iter().flatten().rev().copied());
        }
        for u in &mut self.out.unmappable {
            let n = self.out.nodes.get(u.node).ok_or_else(|| err("unmappable index"))?;
            u.paint_position = n.paint_position;
            u.interval = n.span;
        }
        self.out.unmappable.sort_by_key(|n| n.paint_position);
        self.out.positions = vec![0; self.out.nodes.len()];
        self.out.nodes.sort_by_key(|n| n.paint_position);
        for (pos, n) in self.out.nodes.iter().enumerate() {
            let target = self.out.positions.get_mut(n.index).ok_or_else(|| err("stable node index"))?;
            *target = pos;
        }
        Ok(())
    }
}

#[cfg(test)]
mod depth_tests {
    use super::*;
    use crate::{ExportOptions, Format, ImportOptions, scene};

    #[test]
    fn scene_geometric_depth_is_not_a_stacking_index() {
        // Each of these XSD types declares z as xs:double in scene units.
        // sr-eval/src/program.rs exempts only these four elements from sibling stacking.
        for element in ["camera", "object3D", "particles3D", "ocean"] {
            for value in ["554.256", "-0.5", "1e-3", "2147483648", "-7"] {
                assert_eq!(stacking_z(element, value).unwrap(), 0, "{element} z={value}");
            }
        }
    }

    #[test]
    fn scene_black_hole_depth_truncates_and_saturates_stacking() {
        // blackHoleType/z is xs:double; sr-eval converts it to i32 for sibling order.
        // Fractions truncate toward zero and out-of-range finite values saturate.
        for (value, expected) in [
            ("-7", -7),
            ("-7.9", -7),
            ("7.9", 7),
            ("-0.9", 0),
            ("0.9", 0),
            ("-0", 0),
            ("1e2", 100),
            ("2147483647.9", 2147483647),
            ("-2147483648.9", -2147483648),
            ("1e300", 2147483647),
            ("-1e300", -2147483648),
        ] {
            assert_eq!(stacking_z("blackHole", value).unwrap(), expected, "z={value}");
        }
    }

    #[test]
    fn scene_black_hole_depth_keeps_mixed_sibling_order_and_text() {
        for (value, shape_z, expected) in [
            ("-7", "-1", ["bh", "s"]),
            ("-7.900", "-1", ["bh", "s"]),
            ("7.9", "1", ["s", "bh"]),
            ("-0.9", "0", ["bh", "s"]),
            ("1e300", "2147483647", ["bh", "s"]),
            ("-1e300", "-2147483648", ["bh", "s"]),
        ] {
            // Compare truncated/saturated black-hole z with integer shape z; ties retain
            // document order. The authored geometric depth must retain its exact spelling.
            let text = document(&format!(r#"<blackHole id="bh" mass="1" z="{value}"/><shape id="s" shape="rect" width="8" height="8" z="{shape_z}"/>"#));
            let doc = scene::read::read(&text, &Default::default()).unwrap();
            let errors: Vec<_> = validate::validate(&doc.doc).into_iter().filter(|d| d.is_error()).collect();
            assert!(errors.is_empty(), "fixture must validate: {errors:?}");
            let resolved = resolve(&doc, &Default::default()).unwrap();
            assert_eq!(resolved.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), expected, "z={value}");
            let black_hole = resolved.nodes.iter().find(|n| n.id == "bh").unwrap();
            assert_eq!(black_hole.attributes["z"], value);
        }
    }

    #[test]
    fn scene_depth_types_reject_hostile_values() {
        for element in ["camera", "object3D", "particles3D", "ocean", "blackHole"] {
            // xs:double has no length suffix; scene values must also be finite.
            for value in ["NaN", "INF", "-INF", "1e999", "0.5px", "50%", "1vw", ""] {
                assert!(stacking_z(element, value).is_err(), "{element} accepted {value:?}");
            }
        }
        for element in ["layer", "shape", "group", "sequence", "viewport3D", "particleEmitter"] {
            // nodeCoreAttributes/z is xs:int, including on threeD=true 2D nodes.
            assert_eq!(stacking_z(element, "-7").unwrap(), -7);
            assert_eq!(stacking_z(element, "2147483647").unwrap(), 2147483647);
            assert_eq!(stacking_z(element, "-2147483648").unwrap(), -2147483648);
            for value in ["-0.5", "1.0", "1e2", "2147483648", "-2147483649"] {
                assert!(stacking_z(element, value).is_err(), "{element} accepted {value:?}");
            }
        }
    }

    fn document(nodes: &str) -> String {
        format!(r#"<scene version="1.7"><project width="64" height="64" fps="24" duration="1"/><composition>{nodes}</composition></scene>"#)
    }

    #[test]
    fn scene_fractional_depth_keeps_sibling_paint_order() {
        let text = document(
            r#"<shape id="top" shape="rect" width="8" height="8" threeD="true" z="1"/><camera id="camera" z="554.256"/><object3D id="object" primitive="box" z="-0.5"/><shape id="bottom" shape="rect" width="8" height="8" z="-1"/>"#,
        );
        let doc = scene::read::read(&text, &Default::default()).unwrap();
        let errors: Vec<_> = validate::validate(&doc.doc).into_iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "fixture must validate: {errors:?}");
        let resolved = resolve(&doc, &Default::default()).unwrap();
        // -1 draws first, camera/object both stack at 0 in document order, then +1.
        assert_eq!(resolved.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["bottom", "camera", "object", "top"]);
        assert_eq!(resolved.nodes[1].attributes["z"], "554.256");
        assert_eq!(resolved.nodes[2].attributes["z"], "-0.5");
    }

    #[test]
    fn scene_fractional_depth_survives_overlay_export_and_reimport() {
        for node in [
            r#"<camera id="subject" z="-554.2560000000001"/>"#,
            r#"<object3D id="subject" primitive="box" z="0.125" depth="10.25"/>"#,
            r#"<blackHole id="subject" mass="1" z="-7.900000000000001"/>"#,
        ] {
            // Depth remains its original decimal text in the overlay, with no integer conversion.
            let text = document(node);
            let original = scene::read::read(&text, &Default::default()).unwrap();
            let original = original.doc.descendants().find(|n| n.attribute("id") == Some("subject")).unwrap();
            let (imported, overlays, _) = scene::import(text.as_bytes(), &ImportOptions::default()).unwrap();
            assert_eq!(overlays.len(), 1);
            let overlay = &overlays[0];
            let overlay_text = std::str::from_utf8(&overlay.document).unwrap();
            let overlay_doc = scene::read::read(overlay_text, &Default::default()).unwrap();
            let subject = overlay_doc.doc.descendants().find(|n| n.attribute("id") == Some("subject")).unwrap();
            assert_eq!(subject.attribute("z"), original.attribute("z"));
            assert_eq!(subject.attribute("depth"), original.attribute("depth"));
            let (_, again, _) = scene::import(&overlay.document, &ImportOptions::default()).unwrap();
            let again_text = std::str::from_utf8(&again[0].document).unwrap();
            let again_doc = scene::read::read(again_text, &Default::default()).unwrap();
            let subject = again_doc.doc.descendants().find(|n| n.attribute("id") == Some("subject")).unwrap();
            assert_eq!(subject.attribute("z"), original.attribute("z"));
            assert_eq!(subject.attribute("depth"), original.attribute("depth"));

            // Sequence export references the overlay as media; it does not inline geometric nodes.
            let (exported, _) = crate::export(&imported.project, imported.sequences[0], Format::Scene, &ExportOptions::default()).unwrap();
            let (reimported, _, _) = scene::import(&exported, &ImportOptions::default()).unwrap();
            assert!(reimported.project.items.values().any(|item| {
                item.as_media().is_some_and(|media| matches!(&media.media, filmcraft_project::MediaRef::File { path } if path == &overlay.path))
            }));
        }
    }
}
