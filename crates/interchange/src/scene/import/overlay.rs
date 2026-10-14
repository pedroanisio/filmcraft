//! Overlay planning: the bounded dependency closure that isolates unmappable scene content.
//!
//! The expression tokenizer (`expression_tokens`) is adapted from scene-render
//! `crates/sr-model/src/program_ids.rs` (`tokens`, `rewrite_expression`), commit `9ebe0889`.

use super::super::{
    diag::Collector,
    validate::{IdKind, structure},
};
use super::*;
use roxmltree::NodeId;

const MAX_DEPENDENCY_NODES: usize = 100_000;
const MAX_DEPENDENCY_DEPTH: usize = 256;
const MAX_DEPENDENCY_WORK: usize = 2_000_000;

// XSD nodeBehaviour/bodyBehaviour, with animationElements expanded. The schema
// parity test keeps ancestor behavior retention independent of fixture names.
const BEHAVIOUR_CHILDREN: &[&str] = &["animate", "expression", "motionPath", "link", "mask", "transformConstraint", "rigidBody", "softBody", "deform"];

#[derive(Clone, Copy)]
enum ExpressionDependency {
    Property,
    Id,
    Parameter,
    BeatGrid,
}

// Every document-dependent builtin documented by expressionType.
const EXPRESSION_DEPENDENCIES: &[(&str, ExpressionDependency)] = &[
    ("prop", ExpressionDependency::Property),
    ("propAtTime", ExpressionDependency::Property),
    ("markerTime", ExpressionDependency::Id),
    ("param", ExpressionDependency::Parameter),
    ("audioAmplitude", ExpressionDependency::Id),
    ("geo", ExpressionDependency::Id),
    ("geoVisible", ExpressionDependency::Id),
    ("beat", ExpressionDependency::BeatGrid),
];

pub(super) fn sanitize(value: &str) -> String {
    let result: String = value.chars().take(64).map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') { c } else { '_' }).collect();
    if result.is_empty() { "scene".into() } else { result }
}

fn font_family(value: &str) -> String {
    value.trim().trim_matches(['\'', '"']).split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase()
}

#[derive(Default)]
struct Dependencies {
    structure: structure::Structure,
    fonts: HashMap<String, Vec<NodeId>>,
    beat_grid: Option<NodeId>,
}
impl Dependencies {
    fn new(doc: &SceneDoc<'_>) -> Self {
        let mut collector = Collector::new(&doc.doc);
        let structure = structure::validate(&doc.doc, &mut collector);
        let mut fonts: HashMap<String, Vec<NodeId>> = HashMap::new();
        for font in doc.doc.descendants().filter(|n| n.has_tag_name("font")) {
            if let Some(family) = font.attribute("family") {
                fonts.entry(font_family(family)).or_default().push(font.id());
            }
        }
        let beat_grid =
            doc.root().children().find(|n| n.has_tag_name("markers")).and_then(|n| n.children().find(|n| n.has_tag_name("beatGrid"))).map(|n| n.id());
        Self { structure, fonts, beat_grid }
    }
}

struct Selection<'a, 'i> {
    doc: &'a SceneDoc<'i>,
    dependencies: &'a Dependencies,
    selected: HashSet<NodeId>,
    expanded: HashSet<NodeId>,
    pending: Vec<NodeId>,
    font_families_seen: HashSet<String>,
    enclosing: NodeId,
    work: usize,
    programs_retained: bool,
}
impl Selection<'_, '_> {
    fn spend(&mut self) -> Result<()> {
        self.spend_count(1)
    }
    fn spend_count(&mut self, count: usize) -> Result<()> {
        self.work = self.work.saturating_add(count);
        if self.work > MAX_DEPENDENCY_WORK {
            return Err(err("overlay dependency work limit exceeded"));
        }
        Ok(())
    }
    fn insert(&mut self, n: Node<'_, '_>) -> Result<()> {
        self.spend()?;
        if !self.selected.contains(&n.id()) {
            if self.selected.len() >= MAX_DEPENDENCY_NODES {
                return Err(err("overlay dependency node limit exceeded"));
            }
            self.selected.insert(n.id());
            self.pending.push(n.id());
        }
        Ok(())
    }
    fn select(&mut self, n: Node<'_, '_>, subtree: bool) -> Result<()> {
        let mut depth = 0usize;
        for parent in n.ancestors() {
            if parent.is_element() {
                depth = depth.saturating_add(1);
            }
            if depth > MAX_DEPENDENCY_DEPTH {
                return Err(err("overlay dependency depth limit exceeded"));
            }
            self.insert(parent)?;
        }
        if subtree {
            let mut pending = vec![(n, depth)];
            while let Some((node, depth)) = pending.pop() {
                if depth > MAX_DEPENDENCY_DEPTH {
                    return Err(err("overlay dependency depth limit exceeded"));
                }
                self.insert(node)?;
                if self.expanded.insert(node.id()) {
                    for child in node.children() {
                        self.spend()?;
                        if pending.len() >= MAX_DEPENDENCY_NODES {
                            return Err(err("overlay dependency node limit exceeded"));
                        }
                        pending.push((child, depth.saturating_add(usize::from(child.is_element()))));
                    }
                }
            }
        }
        Ok(())
    }
    fn enclose(&mut self, node: Node<'_, '_>) -> Result<()> {
        let root = self.doc.doc.get_node(self.enclosing).ok_or_else(|| err("overlay root is missing"))?;
        let ancestors: HashSet<_> = root.ancestors().take(MAX_DEPENDENCY_DEPTH.saturating_add(1)).map(|n| n.id()).collect();
        let enclosing = node
            .ancestors()
            .take(MAX_DEPENDENCY_DEPTH.saturating_add(1))
            .find(|n| ancestors.contains(&n.id()))
            .ok_or_else(|| err("overlay dependency depth limit exceeded"))?;
        let mut enclosing = enclosing;
        // Sequence placement shifts descendants and media/affine clocks together.
        // Retain every enclosing sequence rather than rewriting only its direct child.
        for parent in enclosing.ancestors() {
            self.spend()?;
            if parent.has_tag_name("sequence") {
                enclosing = parent;
            }
        }
        self.enclosing = enclosing.id();
        // Keep the original container, including sequence auto-placement and all
        // clocks between the two nodes; copying a dependency as a sibling retimes it.
        self.select(enclosing, true)
    }
    fn reference(&mut self, id: &str) -> Result<()> {
        self.spend()?;
        if let Some(node) = self.dependencies.structure.ids.get(id).and_then(|id| self.doc.doc.get_node(*id)) {
            if node.ancestors().any(|n| n.has_tag_name("composition")) {
                if self.selected.contains(&node.id()) {
                    return Ok(());
                }
                return self.enclose(node);
            }
            return self.select(node, true);
        }
        // Only numeric beat/bar suffixes belong to the grid. A program called
        // `beat` may emit beat.g1, and a literal DOM id takes precedence above.
        if id.strip_prefix("beat.").or_else(|| id.strip_prefix("bar.")).is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())) {
            for grid in self.doc.doc.descendants() {
                self.spend()?;
                if grid.has_tag_name("beatGrid") {
                    return self.select(grid, true);
                }
            }
        }
        if self.programs_retained {
            return Ok(());
        }
        for program in self.doc.doc.descendants() {
            self.spend()?;
            if program.has_tag_name("program")
                && program.attribute("mode").unwrap_or("build").trim() == "build"
                && program.ancestors().any(|p| p.has_tag_name("composition"))
            {
                return self.retain_program_context();
            }
        }
        Err(err(format!("overlay dependency {id} is missing")))
    }
    fn retain_program_context(&mut self) -> Result<()> {
        if !self.programs_retained {
            // Opaque output can reference any original document ID (SREP 87).
            self.programs_retained = true;
            self.enclose(self.doc.root())?;
        }
        Ok(())
    }
    fn expression(&mut self, text: &str) -> Result<()> {
        self.spend_count(text.len())?;
        let tokens = expression_tokens(text)?;
        for (index, token) in tokens.iter().enumerate() {
            self.spend()?;
            let ExprToken::Ident(function) = token else { continue };
            let Some((_, kind)) = EXPRESSION_DEPENDENCIES.iter().find(|(name, _)| *name == function) else { continue };
            let mut next = index.saturating_add(1);
            let mut before = index;
            while tokens.get(next) == Some(&ExprToken::Punct(')')) {
                self.spend()?;
                before = before.checked_sub(1).ok_or_else(|| err("unbalanced expression dependency callee"))?;
                if tokens.get(before) != Some(&ExprToken::Punct('(')) {
                    return Err(err("unbalanced expression dependency callee"));
                }
                next = next.saturating_add(1);
            }
            if before.checked_sub(1).and_then(|i| tokens.get(i)) == Some(&ExprToken::Punct('.')) {
                continue;
            }
            if tokens.get(next) != Some(&ExprToken::Punct('(')) {
                continue;
            }
            if matches!(kind, ExpressionDependency::BeatGrid) {
                if let Some(grid) = self.dependencies.beat_grid.and_then(|id| self.doc.doc.get_node(id)) {
                    self.select(grid, true)?;
                }
                continue;
            }
            next = next.saturating_add(1);
            let mut groups = 0usize;
            while tokens.get(next) == Some(&ExprToken::Punct('(')) {
                self.spend()?;
                next = next.saturating_add(1);
                groups = groups.saturating_add(1);
            }
            let dynamic = || err("overlay has a dynamic expression dependency; cannot preserve its target statically");
            let Some(ExprToken::String(value)) = tokens.get(next) else { return Err(dynamic()) };
            next = next.saturating_add(1);
            for _ in 0..groups {
                self.spend()?;
                if tokens.get(next) != Some(&ExprToken::Punct(')')) {
                    return Err(dynamic());
                }
                next = next.saturating_add(1);
            }
            if !matches!(tokens.get(next), Some(ExprToken::Punct(')' | ','))) {
                return Err(dynamic());
            }
            if matches!(kind, ExpressionDependency::Parameter) {
                let parameter = self
                    .dependencies
                    .structure
                    .ids
                    .get(value)
                    .and_then(|id| self.doc.doc.get_node(*id))
                    .filter(|n| n.has_tag_name("param"))
                    .ok_or_else(|| err("overlay parameter is missing"))?;
                self.select(parameter, true)?;
            } else {
                let id = if matches!(kind, ExpressionDependency::Property) { value.rsplit_once('.').map_or(value.as_str(), |(id, _)| id) } else { value };
                self.reference(id)?;
            }
        }
        Ok(())
    }
    fn font_families(&mut self, value: &str) -> Result<()> {
        for family in value.split(',') {
            self.spend()?;
            let family = font_family(family);
            if !self.font_families_seen.insert(family.clone()) {
                continue;
            }
            if let Some(fonts) = self.dependencies.fonts.get(&family).cloned() {
                for id in fonts {
                    if let Some(font) = self.doc.doc.get_node(id) {
                        self.select(font, true)?;
                    }
                }
            }
        }
        Ok(())
    }
    fn retain_camera_context(&mut self) -> Result<()> {
        let composition = self.doc.root().children().find(|n| n.has_tag_name("composition"));
        // cameraType selects implicitly across the composition; viewport3DType
        // is the only isolated camera scope. Keep the entire affected scope so
        // selection, switching and generated 3D content share one projection.
        for camera in self.doc.doc.descendants() {
            self.spend()?;
            if !camera.has_tag_name("camera") {
                continue;
            }
            let mut context = composition;
            for (depth, parent) in camera.ancestors().enumerate() {
                self.spend()?;
                if depth > MAX_DEPENDENCY_DEPTH {
                    return Err(err("overlay dependency depth limit exceeded"));
                }
                if parent.has_tag_name("viewport3D") {
                    context = Some(parent);
                    break;
                }
            }
            if let Some(context) = context
                && self.selected.contains(&context.id())
                && !self.expanded.contains(&context.id())
            {
                self.enclose(context)?;
            }
        }
        Ok(())
    }
    fn closure(&mut self) -> Result<()> {
        while let Some(id) = self.pending.pop() {
            let n = self.doc.doc.get_node(id).ok_or_else(|| err("overlay node is missing"))?;
            for child in n.children().filter(|c| BEHAVIOUR_CHILDREN.contains(&c.tag_name().name())) {
                self.select(child, true)?;
            }
            if (matches!(n.tag_name().name(), "blackHole" | "accretionDisk")
                || (n.has_tag_name("camera") && matches!(n.attribute("geodesics").map(str::trim), Some("true" | "1"))))
                && let Some(context) = n.ancestors().find(|p| matches!(p.tag_name().name(), "viewport3D" | "composition"))
            {
                self.enclose(context)?;
            }
            if n.has_tag_name("program") && n.attribute("mode").unwrap_or("build").trim() == "build" {
                self.retain_program_context()?;
            }
            if n.has_tag_name("link") {
                let source = n.attribute("source").ok_or_else(|| err("overlay link source is missing"))?;
                let id = if let Some(id) = source.strip_prefix("param:").or_else(|| source.strip_prefix("marker:")) {
                    id
                } else if let Some(track) = source.strip_prefix("audio:") {
                    track.split_once(':').map_or(track, |(id, _)| id)
                } else {
                    source.rsplit_once('.').map(|(id, _)| id).ok_or_else(|| err("invalid overlay link source"))?
                };
                self.reference(id)?;
            }
            if n.has_tag_name("expression") {
                let mut text = String::new();
                for child in n.children() {
                    self.spend()?;
                    if child.is_text() {
                        let part = child.text().unwrap_or("");
                        self.spend_count(part.len())?;
                        text.push_str(part);
                    }
                }
                self.expression(&text)?;
            }
            for attr in n.attributes() {
                self.spend()?;
                if n.has_tag_name("project") && attr.name() == "background" {
                    continue;
                }
                if matches!(attr.name(), "font" | "fallback") {
                    self.font_families(attr.value())?;
                }
                if let Some(reference) = texture_reference(attr.name(), attr.value()) {
                    self.reference(reference)?;
                }
                if n.has_tag_name("object3D") && attr.name() == "materialOverride" {
                    for pair in attr.value().split_whitespace() {
                        let (_, material) = pair.split_once(':').ok_or_else(|| err("overlay materialOverride requires name:id pairs"))?;
                        self.reference(material)?;
                    }
                }
                if n.has_tag_name("object3D") && n.attribute("primitive") == Some("voxels") && attr.name() == "palette" && attr.value().trim() != "file" {
                    for material in attr.value().split_whitespace() {
                        self.reference(material)?;
                    }
                }
                let schema_attr = self.dependencies.structure.types.get(&id).and_then(|t| t.attr(attr.name()));
                if schema_attr.is_some_and(|a| validate::derives_from(a.ty, "expressionString")) {
                    self.expression(attr.value())?;
                }
                let kind = schema_attr.map(|a| a.id);
                if matches!(kind, Some(IdKind::IdRef | IdKind::IdRefs)) {
                    for reference in attr.value().split_whitespace() {
                        self.reference(reference)?;
                    }
                } else if let Some(reference) = attr.value().strip_prefix("url(#").and_then(|s| s.strip_suffix(')')) {
                    self.reference(reference)?;
                } else if matches!(attr.name(), "startMarker" | "endMarker" | "marker") {
                    self.reference(attr.value())?;
                } else if let Some(name) = attr.value().strip_prefix("var(").and_then(|s| s.strip_suffix(')')) {
                    let mut token = None;
                    for candidate in self.doc.doc.descendants() {
                        self.spend()?;
                        if candidate.has_tag_name("token") && candidate.attribute("name") == Some(name) {
                            token = Some(candidate);
                            break;
                        }
                    }
                    let token = token.ok_or_else(|| err("overlay style token is missing"))?;
                    self.select(token, true)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
enum ExprToken {
    Ident(String),
    String(String),
    Punct(char),
    Number,
}

// Adapted from scene-render sr-model/src/program_ids.rs::tokens/rewrite_expression. The caller
// charges input bytes before allocation; malformed tokens and dynamic targets fail closed.
fn expression_tokens(mut rest: &str) -> Result<Vec<ExprToken>> {
    let mut out = Vec::new();
    while let Some(c) = rest.chars().next() {
        let len;
        if c.is_whitespace() {
            len = c.len_utf8();
        } else if rest.starts_with("//") {
            len = rest.find('\n').unwrap_or(rest.len());
        } else if let Some(body) = rest.strip_prefix("/*") {
            len = body.find("*/").ok_or_else(|| err("unterminated expression comment"))?.saturating_add(4);
        } else if matches!(c, '\'' | '"') {
            let mut value = String::new();
            let mut end = None;
            let mut chars = rest.char_indices().skip(1);
            while let Some((offset, ch)) = chars.next() {
                if ch == c {
                    end = Some(offset.saturating_add(ch.len_utf8()));
                    break;
                }
                if ch == '\n' {
                    break;
                }
                if ch == '\\' {
                    let (_, escaped) = chars.next().ok_or_else(|| err("unterminated expression escape"))?;
                    value.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '0' => '\0',
                        other => other,
                    });
                } else {
                    value.push(ch);
                }
            }
            len = end.ok_or_else(|| err("unterminated expression string"))?;
            out.push(ExprToken::String(value));
        } else if c.is_ascii_digit() || (c == '.' && rest.get(1..).is_some_and(|s| s.starts_with(|c: char| c.is_ascii_digit()))) {
            len = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '.')).unwrap_or(rest.len());
            out.push(ExprToken::Number);
        } else if c.is_alphabetic() || matches!(c, '_' | '$') {
            len = rest.find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '$'))).unwrap_or(rest.len());
            out.push(ExprToken::Ident(rest.get(..len).ok_or_else(|| err("expression identifier boundary"))?.into()));
        } else {
            len = c.len_utf8();
            out.push(ExprToken::Punct(c));
        }
        rest = rest.get(len..).ok_or_else(|| err("expression token boundary"))?;
    }
    Ok(out)
}

/// One disjoint overlay and the composition nodes it replaces.
pub(super) struct Plan {
    pub anchor: usize,
    pub root: Option<usize>,
    pub covered: HashSet<usize>,
    pub path: String,
    pub document: Vec<u8>,
    pub has_audio: bool,
}

/// Closes and coalesces dependencies before any native clips are emitted.
pub(super) fn prepare(doc: &SceneDoc<'_>, r: &Resolved, roots: &HashSet<usize>, opts: &ImportOptions) -> Result<Vec<Plan>> {
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    let dependencies = Dependencies::new(doc);
    let mut selections: Vec<Selection<'_, '_>> = Vec::new();
    let mut covered = HashSet::new();
    for node in r.nodes.iter().filter(|n| roots.contains(&n.index)) {
        if covered.contains(&node.xml) {
            continue;
        }
        let selection = select(doc, node, &dependencies)?;
        // Full subtrees are either disjoint or nested. A later promotion may
        // subsume an earlier overlay, including one earlier in paint order.
        selections.retain(|old| !selection.expanded.contains(&old.enclosing));
        covered.extend(selection.expanded.iter().copied());
        selections.push(selection);
    }
    let nodes: HashMap<_, _> = r.nodes.iter().map(|n| (n.xml, n)).collect();
    let mut plans = Vec::new();
    let mut bytes = 0usize;
    for mut selection in selections {
        let root = nodes.get(&selection.enclosing).copied();
        let anchor = root.or_else(|| r.nodes.first()).ok_or_else(|| err("overlay has no composition node"))?;
        let (path, document, has_audio) = document(doc, anchor, opts, &mut selection)?;
        bytes = bytes.saturating_add(document.len());
        if bytes > MAX_OUTPUT {
            return Err(err("aggregate overlay byte limit exceeded"));
        }
        plans.push(Plan {
            anchor: anchor.index,
            root: root.map(|n| n.index),
            covered: r.nodes.iter().filter(|n| selection.expanded.contains(&n.xml)).map(|n| n.index).collect(),
            path,
            document,
            has_audio,
        });
    }
    Ok(plans)
}

fn select<'a, 'i>(doc: &'a SceneDoc<'i>, n: &ResolvedNode, dependencies: &'a Dependencies) -> Result<Selection<'a, 'i>> {
    let root = xml_node(doc, n)?;
    let mut selection = Selection {
        doc,
        dependencies,
        selected: HashSet::new(),
        expanded: HashSet::new(),
        pending: Vec::new(),
        font_families_seen: HashSet::new(),
        enclosing: root.id(),
        work: 0,
        programs_retained: false,
    };
    selection.enclose(root)?;
    selection.retain_camera_context()?;
    if let Some(project) = doc.root().children().find(|n| n.has_tag_name("project")) {
        selection.select(project, true)?;
    }
    if let Some(color) = doc.root().children().find(|n| n.has_tag_name("colorManagement")) {
        selection.select(color, true)?;
    }
    selection.closure()?;
    Ok(selection)
}

/// Serializes a dependency-closed overlay on the original scene clocks.
fn document(doc: &SceneDoc<'_>, n: &ResolvedNode, opts: &ImportOptions, selection: &mut Selection<'_, '_>) -> Result<(String, Vec<u8>, bool)> {
    let mut output = String::new();
    serialize(doc.root(), selection, opts, &mut output)?;
    let parsed = read::read(&output, &Default::default())?;
    let diags = validate::validate(&parsed.doc);
    if diags.iter().any(|d| d.is_error()) {
        return Err(Error::Other(format!(
            "generated scene overlay failed validation: {}",
            diags.iter().filter(|d| d.is_error()).map(ToString::to_string).collect::<Vec<_>>().join("; ")
        )));
    }
    let stem = opts.name.as_deref().map(common::file_stem).unwrap_or("Scene");
    let stem: String = stem.chars().take(128).map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ' ') { c } else { '_' }).collect();
    let relative = format!("{} Scene Overlays/{}-{}.scene", if stem.is_empty() { "Scene" } else { &stem }, n.paint_position, sanitize(&n.id));
    let path = safe_path(&relative, opts.base_dir.as_deref())?;
    let has_audio = selection
        .selected
        .iter()
        .filter_map(|id| doc.doc.get_node(*id))
        .any(|n| n.has_tag_name("audio") || (n.has_tag_name("video") && matches!(n.attribute("hasAudio"), Some("true" | "1"))));
    Ok((path, output.into_bytes(), has_audio))
}

fn file_attribute(structure: &structure::Structure, node: Node<'_, '_>, name: &str) -> bool {
    let Some(ty) = structure.types.get(&node.id()) else { return false };
    let value = node.attribute(name).unwrap_or("");
    if ty.name == "imageSequenceAssetType" && name == "src" {
        return true;
    }
    if ty.name == "basemapType" && name == "mapStyle" {
        return !matches!(value, "protomaps-light" | "protomaps-dark");
    }
    if ty.name == "captionTrackType" && name == "model" {
        // Resolving a bare provider model name requires I/O. Explicit paths and ggml
        // filenames are relocatable; names such as base.en remain provider names.
        return value.contains(['/', '\\']) || value.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("bin"));
    }
    ty.attr(name).is_some_and(|a| validate::root_builtin(a.ty) == Some(validate::Builtin::AnyUri) || validate::derives_from(a.ty, "volumeSourceType"))
}

fn texture_reference<'a>(name: &str, value: &'a str) -> Option<&'a str> {
    if matches!(name, "baseColorMap" | "normalMap" | "metallicRoughnessMap" | "occlusionMap" | "emissiveMap" | "displacementMap") {
        value.strip_prefix('#')
    } else {
        None
    }
}

fn remote_uri(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && !scheme.eq_ignore_ascii_case("file")
            && scheme.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
            && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    })
}

fn push(out: &mut String, s: &str) -> Result<()> {
    if out.len().saturating_add(s.len()) > MAX_OUTPUT {
        return Err(err("overlay document byte limit exceeded"));
    }
    out.push_str(s);
    Ok(())
}
fn escaped(out: &mut String, text: &str) -> Result<()> {
    for c in text.chars() {
        match c {
            '&' => push(out, "&amp;")?,
            '<' => push(out, "&lt;")?,
            '>' => push(out, "&gt;")?,
            '"' => push(out, "&quot;")?,
            '\r' => push(out, "&#13;")?,
            '\n' => push(out, "&#10;")?,
            '\t' => push(out, "&#9;")?,
            _ => {
                let mut buf = [0u8; 4];
                push(out, c.encode_utf8(&mut buf))?;
            }
        }
    }
    Ok(())
}
fn serialize(root: Node<'_, '_>, selection: &mut Selection<'_, '_>, opts: &ImportOptions, out: &mut String) -> Result<()> {
    let mut pending = vec![(root, 0usize, false)];
    while let Some((n, depth, close)) = pending.pop() {
        selection.spend()?;
        if depth > MAX_DEPENDENCY_DEPTH {
            return Err(err("overlay nesting limit exceeded"));
        }
        if !selection.selected.contains(&n.id()) {
            continue;
        }
        if n.is_text() {
            escaped(out, n.text().unwrap_or(""))?;
            continue;
        }
        if !n.is_element() {
            continue;
        }
        let tag = n.tag_name().name();
        if close {
            push(out, "</")?;
            push(out, tag)?;
            push(out, ">")?;
            continue;
        }
        push(out, "<")?;
        push(out, tag)?;
        for a in n.attributes() {
            selection.spend()?;
            if tag == "project" && a.name() == "background" {
                continue;
            }
            if a.namespace().is_some() {
                continue;
            }
            push(out, " ")?;
            push(out, a.name())?;
            push(out, "=\"")?;
            if file_attribute(&selection.dependencies.structure, n, a.name())
                && texture_reference(a.name(), a.value()).is_none()
                && !a.value().is_empty()
                && !remote_uri(a.value())
            {
                let path = safe_path(a.value(), opts.base_dir.as_deref())?;
                if common::is_absolute(&path) || path.contains("://") {
                    escaped(out, &path)?;
                } else {
                    let relative = safe_path(a.value(), None)?;
                    escaped(out, &format!("../{relative}"))?;
                }
            } else {
                escaped(out, a.value())?;
            }
            push(out, "\"")?;
        }
        if tag == "project" {
            push(out, " background=\"#00000000\"")?;
        }
        push(out, ">")?;
        pending.push((n, depth, true));
        for child in n.children().rev() {
            selection.spend()?;
            if selection.selected.contains(&child.id()) {
                if pending.len() >= MAX_DEPENDENCY_NODES.saturating_mul(2) {
                    return Err(err("overlay serialization node limit exceeded"));
                }
                pending.push((child, depth.saturating_add(1), false));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(content: &str) -> String {
        format!(r#"<scene version="1.7"><project width="64" height="64" fps="24" duration="12"/>{content}</scene>"#)
    }

    fn fragment(xml: &str, id: &str) -> Result<String> {
        let doc = read::read(xml, &Default::default())?;
        assert!(validate::validate(&doc.doc).iter().all(|d| !d.is_error()), "invalid test fixture: {:?}", validate::validate(&doc.doc));
        let resolved = resolve::resolve(&doc, &Default::default())?;
        let node = resolved.nodes.iter().find(|n| n.id == id).unwrap();
        let dependencies = Dependencies::new(&doc);
        let mut selection = select(&doc, node, &dependencies)?;
        let (_, bytes, _) = document(&doc, node, &Default::default(), &mut selection)?;
        Ok(String::from_utf8(bytes).unwrap())
    }

    #[test]
    fn overlay_external_dependency_keeps_enclosing_clocks() {
        // sr-eval Clock::Affine: child = origin + (t - origin - offset) * scale.
        // With origin=2, offset=2, scale=2: child=2t-6; [4,8) is global [5,7).
        let xml = scene(
            r#"<composition><group id="clock" start="2" end="10" timeScale="2" timeOffset="2"><shape id="source" shape="rect" width="16" height="16" start="4" end="8"><animate property="x"><key time="0" value="0"/><key time="1" value="16"/></animate></shape><shape id="subject" shape="rect" width="16" height="16"><expression property="x">propAtTime('source.x', 0.5)</expression></shape></group></composition>"#,
        );
        let output = fragment(&xml, "subject").unwrap();
        let doc = read::read(&output, &Default::default()).unwrap();
        let resolved = resolve::resolve(&doc, &Default::default()).unwrap();
        let source = resolved.nodes.iter().find(|n| n.id == "source").unwrap();
        assert_eq!((source.interval.start, source.interval.end), (5., 7.));
        assert!(output.contains("propAtTime('source.x', 0.5)"));
        assert!(output.contains("value=\"16\""));
    }

    #[test]
    fn overlay_generated_reference_keeps_all_build_emitters() {
        // SREP 87: bare g1 may be emitted by either build program. Keep both so
        // post-build ambiguity checking still sees both; do not execute either.
        for reference in ["g1.x", "a.g1.x", "beat.g1.x"] {
            let xml = scene(&format!(
                r#"<composition><program id="a" src="a.wasm" sha256="{hash}"/><program id="beat" src="b.wasm" sha256="{hash}"/><shape id="subject" shape="rect" width="16" height="16"><expression property="x">prop('{reference}')</expression></shape></composition>"#,
                hash = "0".repeat(64)
            ));
            let output = fragment(&xml, "subject").unwrap();
            assert!(output.contains("id=\"a\""));
            assert!(output.contains("id=\"beat\""));
            assert!(output.contains(reference));
        }
    }

    #[test]
    fn overlay_missing_reference_is_not_deferred_to_step_program() {
        let xml = scene(&format!(
            r#"<composition><program id="stepper" mode="step" width="64" height="64" src="a.wasm" sha256="{}"/><shape id="subject" shape="rect" width="16" height="16"><expression property="x">prop('missing.x')</expression></shape></composition>"#,
            "0".repeat(64)
        ));
        assert!(fragment(&xml, "subject").unwrap_err().to_string().contains("missing"));
    }

    #[test]
    fn overlay_generated_marker_keeps_grid_and_explicit_marker_wins() {
        // beatGrid generates beat.N/bar.N; an explicit id shadows its generated marker.
        for markers in [r#"<beatGrid bpm="120"/>"#, r#"<marker id="beat.2" time="3"/>"#] {
            let xml = scene(&format!(
                r#"<markers>{markers}</markers><composition><shape id="subject" shape="rect" width="16" height="16"><expression property="x">markerTime('beat.2')</expression></shape></composition>"#
            ));
            let output = fragment(&xml, "subject").unwrap();
            assert!(output.contains(markers.trim_end_matches("/>").trim_start_matches('<')));
        }
    }

    #[test]
    fn overlay_string_material_dependencies_are_transitive() {
        // object3DType: override pairs and voxel palette tokens name materials;
        // material maps use #image ids, so both paint and texture must survive.
        for (asset, attrs) in [
            (r#"<mesh id="model" src="model.glb" format="glb"/>"#, r#"primitive="mesh" mesh="model" materialOverride="surface:paint""#),
            (r#"<voxelAsset id="model" src="model.vox" maxCells="8"/>"#, r#"primitive="voxels" voxels="model" palette="paint""#),
        ] {
            let xml = scene(&format!(
                r##"<assets>{asset}<image id="texture" src="paint.png" width="16" height="16"/><image id="unused" src="unused.png" width="16" height="16"/></assets><materials><material id="paint" baseColorMap="#texture"/></materials><composition><object3D id="subject" {attrs}/></composition>"##
            ));
            let output = fragment(&xml, "subject").unwrap();
            assert!(output.contains("id=\"paint\""));
            assert!(output.contains("id=\"texture\""));
            assert!(!output.contains("unused.png"));
        }
    }

    #[test]
    fn overlay_voxel_file_palette_is_not_an_id() {
        let xml = scene(
            r#"<assets><voxelAsset id="model" src="model.vox" maxCells="8"/></assets><composition><object3D id="subject" primitive="voxels" voxels="model" palette="file"/></composition>"#,
        );
        assert!(fragment(&xml, "subject").is_ok());
    }

    #[test]
    fn overlay_geodesic_context_is_kept_from_either_root() {
        // cameraType/geodesics renders the hole and its disk with the 2D background.
        let xml = scene(
            r#"<composition><camera id="view" z="-10" geodesics="true"/><blackHole id="hole" mass="1"/><accretionDisk id="disk" blackHole="hole" outerRadius="12" temperatureScale="1000"/><shape id="sky" shape="rect" width="16" height="16"/></composition>"#,
        );
        for id in ["view", "hole", "disk"] {
            let output = fragment(&xml, id).unwrap();
            for expected in ["view", "hole", "disk", "sky"] {
                assert!(output.contains(&format!("id=\"{expected}\"")));
            }
        }
    }
    #[test]
    fn overlay_review_sequence_descendants_keep_placement() {
        // sr-eval/program.rs::shift moves the timeline-sharing subtree by 3s:
        // g and both children must remain [3,5), not [0,2) clipped by g.
        let xml = scene(
            r#"<composition><sequence id="s"><shape id="first" shape="rect" width="8" height="8" end="3"/><group id="g" end="2"><shape id="other" shape="rect" width="8" height="8" end="2"/><shape id="subject" shape="rect" width="8" height="8" end="2"><expression property="x">prop('other.x')</expression></shape></group></sequence></composition>"#,
        );
        let output = fragment(&xml, "subject").unwrap();
        let doc = read::read(&output, &Default::default()).unwrap();
        let r = resolve::resolve(&doc, &Default::default()).unwrap();
        for id in ["g", "other", "subject"] {
            let n = r.nodes.iter().find(|n| n.id == id).unwrap();
            assert_eq!((n.interval.start, n.interval.end), (3., 5.), "{id}");
        }
    }

    #[test]
    fn overlay_review_build_output_can_reference_document_resources() {
        // SREP 87 resolves output references against original document IDs.
        // An opaque emitter can use picture even without an original IDREF to it.
        for expression in ["prop('p.g1.x')", "0.5"] {
            let xml = scene(&format!(
                r#"<assets><image id="picture" src="picture.png" width="8" height="8"/></assets><composition><program id="p" src="p.wasm" sha256="{}"/><shape id="subject" shape="rect" width="8" height="8"><expression property="x">{expression}</expression></shape></composition>"#,
                "0".repeat(64)
            ));
            let id = if expression == "0.5" { "p" } else { "subject" };
            assert!(fragment(&xml, id).unwrap().contains("id=\"picture\""));
        }
    }

    #[test]
    fn overlay_review_expression_attribute_parameter() {
        // condition derives from expressionString and reads the document parameter.
        let xml = scene(
            r#"<parameters><param id="show" type="number" default="1"/></parameters><composition><object3D id="subject" primitive="box" condition="param('show')"/></composition>"#,
        );
        assert!(fragment(&xml, "subject").unwrap().contains("id=\"show\""));
    }

    #[test]
    fn overlay_review_parametric_expression_attribute() {
        // parametricPath x/y are expressionString, not numeric coordinates.
        let xml = scene(
            r#"<composition><shape id="other" shape="rect" width="8" height="8"/><shape id="subject" shape="parametric"><parametricPath x="prop('other.x')" y="t"/></shape></composition>"#,
        );
        assert!(fragment(&xml, "subject").unwrap().contains("id=\"other\""));
    }

    #[test]
    fn overlay_review_link_sources() {
        // linkType documents node.property, param:name, marker:id and audio:track:band.
        for (source, expected) in [("other.x", "other"), ("param:amount", "amount"), ("marker:cue", "cue"), ("audio:music:low", "music")] {
            let xml = scene(&format!(
                r#"<parameters><param id="amount" type="number" default="1"/></parameters><assets><audio id="sound" src="music.wav" duration="12"/></assets><markers><marker id="cue" time="2"/></markers><composition><shape id="other" shape="rect" width="8" height="8"/><object3D id="subject" primitive="box"><link property="x" source="{source}"/></object3D></composition><audioMix><audioTrack id="music" asset="sound"/></audioMix>"#
            ));
            assert!(fragment(&xml, "subject").unwrap().contains(&format!("id=\"{expected}\"")), "{source}");
        }
    }

    #[test]
    fn overlay_review_ancestor_link() {
        let xml = scene(
            r#"<parameters><param id="amount" type="number" default="1"/></parameters><composition><group id="g"><link property="x" source="param:amount"/><object3D id="subject" primitive="box"/></group></composition>"#,
        );
        let output = fragment(&xml, "subject").unwrap();
        assert!(output.contains("<link"));
        assert!(output.contains("id=\"amount\""));
    }

    #[test]
    fn overlay_review_grouped_expression_calls() {
        // sr-model/program_ids.rs and sr-eval allow grouping of callee and argument.
        for expr in ["(prop)('other.x')", "((prop)) /* comment */ (( 'other.x' ))", "prop /* comment */ ('other.x')", "prop('other\\.x')"] {
            let xml = scene(&format!(
                r#"<composition><shape id="other" shape="rect" width="8" height="8"/><object3D id="subject" primitive="box"><expression property="x">{expr}</expression></object3D></composition>"#
            ));
            assert!(fragment(&xml, "subject").unwrap().contains("id=\"other\""), "{expr}");
        }
    }

    #[test]
    fn overlay_review_dynamic_expression_rejected() {
        for expr in ["(prop)(target)", "prop('other.' + target)", "propAtTime(('other.' + target), 0)"] {
            let xml = scene(&format!(
                r#"<composition><shape id="other" shape="rect" width="8" height="8"/><object3D id="subject" primitive="box"><expression property="x">{expr}</expression></object3D></composition>"#
            ));
            assert!(fragment(&xml, "subject").unwrap_err().to_string().contains("dynamic expression dependency"), "{expr}");
        }
    }

    #[test]
    fn overlay_review_expression_comments_are_not_calls() {
        let xml = scene(
            r#"<composition><object3D id="subject" primitive="box"><expression property="x">/* prop(target) */ 1 // param(missing)
        </expression></object3D></composition>"#,
        );
        assert!(fragment(&xml, "subject").is_ok());
    }

    #[test]
    fn overlay_review_serialization_charges_work() {
        let xml = scene(r#"<composition><object3D id="subject" primitive="box"/></composition>"#);
        let doc = read::read(&xml, &Default::default()).unwrap();
        let r = resolve::resolve(&doc, &Default::default()).unwrap();
        let node = r.nodes.iter().find(|n| n.id == "subject").unwrap();
        let deps = Dependencies::new(&doc);
        let mut selected = select(&doc, node, &deps).unwrap();
        selected.work = MAX_DEPENDENCY_WORK;
        assert!(document(&doc, node, &Default::default(), &mut selected).unwrap_err().to_string().contains("work limit"));
    }

    #[test]
    fn overlay_review_wide_sequence_serialization_is_linear() {
        // A retained 20,000-child sequence must visit nodes once, not rescan
        // all siblings per child (400 million membership checks).
        let xml = scene(&format!(
            "<composition><sequence id=\"subject\">{}</sequence></composition>",
            (0..20_000).map(|i| format!("<group id=\"g{i}\"/>")).collect::<String>()
        ));
        let doc = read::read(&xml, &Default::default()).unwrap();
        let r = resolve::resolve(&doc, &Default::default()).unwrap();
        let node = r.nodes.iter().find(|n| n.id == "subject").unwrap();
        let deps = Dependencies::new(&doc);
        let mut selected = select(&doc, node, &deps).unwrap();
        selected.work = 0;
        document(&doc, node, &Default::default(), &mut selected).unwrap();
        assert!((20_000..500_000).contains(&selected.work), "serialization must charge linear work: {}", selected.work);
    }

    #[test]
    fn overlay_round3_xml_comments_preserve_all_expression_text() {
        // XML comments contribute no characters: the expression is 0 + prop('other.x').
        let xml = scene(
            r#"<composition><shape id="other" shape="rect" width="8" height="8" x="100"/><object3D id="subject" primitive="box"><expression property="x">0 + <!-- gap --> prop('other.x')</expression></object3D></composition>"#,
        );
        assert!(fragment(&xml, "subject").unwrap().contains("id=\"other\""));
    }

    #[test]
    fn overlay_round3_ancestor_transform_constraint() {
        // copy-position inherits other's world x=100; target must survive transitively.
        let xml = scene(
            r#"<composition><shape id="other" shape="rect" width="8" height="8" x="100"/><group id="g"><transformConstraint type="copy-position" target="other"/><object3D id="subject" primitive="box"/></group></composition>"#,
        );
        let output = fragment(&xml, "subject").unwrap();
        assert!(output.contains("<transformConstraint"));
        assert!(output.contains("id=\"other\""));
    }

    fn builtin_fragment(expression: &str) -> String {
        let xml = scene(&format!(
            r#"<assets><audio id="sound" src="music.wav" duration="12"/><geo id="land" src="land.geojson"/><map id="world" width="64" height="64"><geoLayer geo="land"/></map></assets><markers><beatGrid bpm="120" source="music"/></markers><composition><object3D id="subject" primitive="box"><expression property="x">{expression}</expression></object3D></composition><audioMix><audioTrack id="music" asset="sound"/></audioMix>"#
        ));
        fragment(&xml, "subject").unwrap()
    }

    #[test]
    fn overlay_round3_beat_grid_and_source() {
        // XSD beatGrid: beat()=time*120/60, unless source supplies detected beats.
        let output = builtin_fragment("10 * beat()");
        assert!(output.contains("<beatGrid"));
        assert!(output.contains("id=\"music\""));
        assert!(output.contains("id=\"sound\""));
    }

    #[test]
    fn overlay_round3_audio_amplitude_track() {
        let output = builtin_fragment("audioAmplitude('music', 'low')");
        assert!(output.contains("id=\"music\""));
        assert!(output.contains("id=\"sound\""));
    }

    #[test]
    fn overlay_round3_geo_map() {
        let output = builtin_fragment("geo('world', 0, 0).x");
        assert!(output.contains("id=\"world\""));
        assert!(output.contains("id=\"land\""));
    }

    #[test]
    fn overlay_round3_geo_visible_map() {
        assert!(builtin_fragment("geoVisible('world', 0, 0)").contains("id=\"world\""));
    }

    #[test]
    fn overlay_round3_dynamic_builtin_targets_refused() {
        for expression in ["audioAmplitude(target)", "geo(target, 0, 0)", "geoVisible(target, 0, 0)"] {
            let xml = scene(&format!(
                r#"<composition><object3D id="subject" primitive="box"><expression property="x">{expression}</expression></object3D></composition>"#
            ));
            assert!(fragment(&xml, "subject").is_err(), "{expression}");
        }
    }

    #[test]
    fn overlay_round3_behaviour_table_matches_schema_groups() {
        let xsd = roxmltree::Document::parse(include_str!("../../../schema/sr-core/scene-render.xsd")).unwrap();
        let mut pending = vec!["nodeBehaviour", "bodyBehaviour"];
        let mut seen = HashSet::new();
        let mut elements = HashSet::new();
        while let Some(name) = pending.pop() {
            assert!(seen.insert(name), "cyclic schema group");
            let group = xsd.root_element().children().find(|n| n.tag_name().name() == "group" && n.attribute("name") == Some(name)).unwrap();
            for n in group.descendants() {
                match n.tag_name().name() {
                    "element" => {
                        elements.insert(n.attribute("name").unwrap());
                    }
                    "group" => {
                        if let Some(reference) = n.attribute("ref") {
                            pending.push(reference);
                        }
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(BEHAVIOUR_CHILDREN.iter().copied().collect::<HashSet<_>>(), elements);
    }

    fn selection<'a, 'i>(doc: &'a SceneDoc<'i>, dependencies: &'a Dependencies) -> Selection<'a, 'i> {
        Selection {
            doc,
            dependencies,
            selected: HashSet::new(),
            expanded: HashSet::new(),
            pending: Vec::new(),
            font_families_seen: HashSet::new(),
            enclosing: doc.root().id(),
            work: 0,
            programs_retained: false,
        }
    }

    #[test]
    fn overlay_dependency_node_budget() {
        // Document + scene + project + composition = four nodes before the groups.
        let dependencies = Dependencies::default();
        for groups in [99_996, 99_997] {
            let xml = scene(&format!("<composition>{}</composition>", "<group/>".repeat(groups)));
            let doc = read::read(&xml, &Default::default()).unwrap();
            assert_eq!(doc.doc.descendants().count(), groups + 4);
            let result = selection(&doc, &dependencies).select(doc.root(), true);
            assert_eq!(result.is_ok(), groups == 99_996, "selection must cap unique nodes at 100000");
        }
    }

    #[test]
    fn overlay_dependency_depth_budget() {
        // scene depth 1, composition depth 2: 254 groups fit the 256 limit.
        let dependencies = Dependencies::default();
        for groups in [254, 255] {
            let xml = scene(&format!("<composition>{}{}</composition>", "<group>".repeat(groups), "</group>".repeat(groups)));
            let doc = read::read(&xml, &read::Limits { max_depth: 300, ..Default::default() }).unwrap();
            assert_eq!(selection(&doc, &dependencies).select(doc.root(), true).is_ok(), groups == 254, "closure must enforce its own depth budget");
        }
    }

    #[test]
    fn overlay_dependency_work_budget() {
        let dependencies = Dependencies::default();
        let xml = scene("<composition/>");
        let doc = read::read(&xml, &Default::default()).unwrap();
        let mut selection = selection(&doc, &dependencies);
        selection.work = 2_000_000;
        assert!(selection.select(doc.root(), true).is_err(), "even a repeated selection must charge work");
    }
    #[test]
    fn overlay_round4_implicit_camera_dependency_from_either_root() {
        // cameraType selects view implicitly for the entire composition, even
        // when the object and camera are in different ordinary groups.
        let xml = scene(r#"<composition><group id="rig"><camera id="view" z="-1000" fov="20"/></group><object3D id="box" primitive="box"/></composition>"#);
        for id in ["view", "box"] {
            let output = fragment(&xml, id).unwrap();
            let doc = read::read(&output, &Default::default()).unwrap();
            for expected in ["view", "box"] {
                assert!(doc.doc.descendants().any(|n| n.attribute("id") == Some(expected)), "root {id} omitted {expected}");
            }
        }
    }
    #[test]
    fn overlay_round4_viewport_camera_stays_isolated() {
        // viewport3DType owns its camera: it must project box, never world.
        let xml = scene(
            r#"<composition><viewport3D id="vp" width="64" height="64"><camera id="view" z="-1000"/><object3D id="box" primitive="box"/></viewport3D><object3D id="world" primitive="box"/></composition>"#,
        );
        for id in ["view", "box"] {
            let output = fragment(&xml, id).unwrap();
            assert!(output.contains("id=\"view\""));
            assert!(output.contains("id=\"box\""));
            assert!(!output.contains("id=\"world\""));
        }
        assert!(!fragment(&xml, "world").unwrap().contains("id=\"view\""));
    }
}
