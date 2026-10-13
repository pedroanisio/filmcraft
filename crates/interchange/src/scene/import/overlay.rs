use super::super::{
    diag::Collector,
    validate::{IdKind, structure},
};
use super::*;
use roxmltree::NodeId;

pub(super) fn sanitize(value: &str) -> String {
    let result: String = value.chars().take(64).map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') { c } else { '_' }).collect();
    if result.is_empty() { "scene".into() } else { result }
}

fn font_family(value: &str) -> String {
    value.trim().trim_matches(['\'', '"']).split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase()
}

struct Selection<'a, 'i> {
    doc: &'a SceneDoc<'i>,
    structure: structure::Structure,
    selected: HashSet<NodeId>,
    expanded: HashSet<NodeId>,
    pending: Vec<NodeId>,
    fonts: HashMap<String, Vec<NodeId>>,
    font_families_seen: HashSet<String>,
}
impl Selection<'_, '_> {
    fn select(&mut self, n: Node<'_, '_>, subtree: bool) {
        if subtree && self.expanded.insert(n.id()) {
            for child in n.descendants() {
                if self.selected.insert(child.id()) {
                    self.pending.push(child.id());
                }
            }
        }
        for parent in n.ancestors() {
            if self.selected.insert(parent.id()) {
                self.pending.push(parent.id());
            }
        }
    }
    fn reference(&mut self, id: &str) -> Result<()> {
        let Some(node) = self.structure.ids.get(id).and_then(|id| self.doc.doc.get_node(*id)) else {
            return Err(err(format!("overlay dependency {id} is missing")));
        };
        if node.ancestors().any(|n| n.has_tag_name("composition")) {
            if self.selected.contains(&node.id()) {
                return Ok(());
            }
            return Err(err(format!("overlay dependency {id} is outside its subtree; cannot isolate external composition dependency")));
        }
        self.select(node, true);
        Ok(())
    }
    fn expression(&mut self, text: &str) -> Result<()> {
        for function in ["prop", "propAtTime", "markerTime", "param"] {
            let mut rest = text;
            while let Some(offset) = rest.find(function) {
                rest = rest.get(offset.saturating_add(function.len())..).ok_or_else(|| err("expression boundary"))?.trim_start();
                let Some(args) = rest.strip_prefix('(') else {
                    continue;
                };
                let args = args.trim_start();
                let quote = args
                    .chars()
                    .next()
                    .filter(|c| matches!(c, '\'' | '"'))
                    .ok_or_else(|| err("overlay has a dynamic expression dependency; isolate it in a group first"))?;
                let body = args.get(1..).ok_or_else(|| err("expression dependency string"))?;
                let end = body.find(quote).ok_or_else(|| err("unterminated expression dependency"))?;
                let value = body.get(..end).ok_or_else(|| err("expression dependency boundary"))?;
                if value.contains('\\') {
                    return Err(err("escaped expression dependency cannot be isolated"));
                }
                if function == "param" {
                    let parameter = self
                        .doc
                        .doc
                        .descendants()
                        .find(|n| n.has_tag_name("param") && n.attribute("id") == Some(value))
                        .ok_or_else(|| err("overlay parameter is missing"))?;
                    self.select(parameter, true);
                } else {
                    let id = if function == "markerTime" { value } else { value.rsplit_once('.').map_or(value, |(id, _)| id) };
                    self.reference(id)?;
                }
                rest = body.get(end.saturating_add(1)..).ok_or_else(|| err("expression dependency boundary"))?;
            }
        }
        Ok(())
    }
    fn font_families(&mut self, value: &str) {
        for family in value.split(',') {
            let family = font_family(family);
            if !self.font_families_seen.insert(family.clone()) {
                continue;
            }
            if let Some(fonts) = self.fonts.get(&family).cloned() {
                for id in fonts {
                    if let Some(font) = self.doc.doc.get_node(id) {
                        self.select(font, true);
                    }
                }
            }
        }
    }
    fn closure(&mut self) -> Result<()> {
        while let Some(id) = self.pending.pop() {
            let n = self.doc.doc.get_node(id).ok_or_else(|| err("overlay node is missing"))?;
            if n.has_tag_name("expression") {
                self.expression(n.text().unwrap_or(""))?;
            }
            for attr in n.attributes() {
                if n.has_tag_name("project") && attr.name() == "background" {
                    continue;
                }
                if matches!(attr.name(), "font" | "fallback") {
                    self.font_families(attr.value());
                }
                if let Some(reference) = texture_reference(attr.name(), attr.value()) {
                    self.reference(reference)?;
                }
                let kind = self.structure.types.get(&id).and_then(|t| t.attr(attr.name())).map(|a| a.id);
                if matches!(kind, Some(IdKind::IdRef | IdKind::IdRefs)) {
                    for reference in attr.value().split_whitespace() {
                        self.reference(reference)?;
                    }
                } else if let Some(reference) = attr.value().strip_prefix("url(#").and_then(|s| s.strip_suffix(')')) {
                    self.reference(reference)?;
                } else if matches!(attr.name(), "startMarker" | "endMarker" | "marker") {
                    if attr.value().starts_with("beat.") || attr.value().starts_with("bar.") {
                        if let Some(grid) = self.doc.doc.descendants().find(|n| n.has_tag_name("beatGrid")) {
                            self.select(grid, true);
                        }
                    } else {
                        self.reference(attr.value())?;
                    }
                } else if let Some(name) = attr.value().strip_prefix("var(").and_then(|s| s.strip_suffix(')')) {
                    let token = self
                        .doc
                        .doc
                        .descendants()
                        .find(|n| n.has_tag_name("token") && n.attribute("name") == Some(name))
                        .ok_or_else(|| err("overlay style token is missing"))?;
                    self.select(token, true);
                }
            }
        }
        Ok(())
    }
}

pub(super) fn document(doc: &SceneDoc<'_>, r: &Resolved, n: &ResolvedNode, opts: &ImportOptions) -> Result<(String, Vec<u8>, bool)> {
    let root = xml_node(doc, n)?;
    let mut collector = Collector::new(&doc.doc);
    let structure = structure::validate(&doc.doc, &mut collector);
    let mut fonts: HashMap<String, Vec<NodeId>> = HashMap::new();
    for font in doc.doc.descendants().filter(|n| n.has_tag_name("font")) {
        if let Some(family) = font.attribute("family") {
            fonts.entry(font_family(family)).or_default().push(font.id());
        }
    }
    let mut selection =
        Selection { doc, structure, selected: HashSet::new(), expanded: HashSet::new(), pending: Vec::new(), fonts, font_families_seen: HashSet::new() };
    selection.select(root, true);
    for ancestor in root.ancestors().filter(Node::is_element) {
        // Animated ancestor properties continue to run on their original clocks.
        for child in ancestor.children().filter(|c| matches!(c.tag_name().name(), "animate" | "expression" | "motionPath" | "mask")) {
            selection.select(child, true);
        }
    }
    if let Some(project) = doc.root().children().find(|n| n.has_tag_name("project")) {
        selection.select(project, true);
    }
    if let Some(color) = doc.root().children().find(|n| n.has_tag_name("colorManagement")) {
        selection.select(color, true);
    }
    selection.closure()?;
    let mut output = String::new();
    let nodes: HashMap<_, _> = r.nodes.iter().map(|n| (n.xml, n)).collect();
    serialize(doc.root(), &selection, &nodes, opts, &mut output, 0)?;
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
fn serialize(
    n: Node<'_, '_>,
    selection: &Selection<'_, '_>,
    nodes: &HashMap<NodeId, &ResolvedNode>,
    opts: &ImportOptions,
    out: &mut String,
    depth: usize,
) -> Result<()> {
    let selected = &selection.selected;
    if !selected.contains(&n.id()) {
        return Ok(());
    }
    if depth > 256 {
        return Err(err("overlay nesting limit exceeded"));
    }
    if n.is_text() {
        return escaped(out, n.text().unwrap_or(""));
    }
    if !n.is_element() {
        return Ok(());
    }
    let original = n.tag_name().name();
    let pruned_sequence = |node: Node<'_, '_>| node.has_tag_name("sequence") && node.children().filter(Node::is_element).any(|c| !selected.contains(&c.id()));
    let rewritten = pruned_sequence(n);
    let tag = if rewritten { "group" } else { original };
    let placed = n.parent_element().filter(|p| pruned_sequence(*p)).and_then(|_| nodes.get(&n.id()));
    push(out, "<")?;
    push(out, tag)?;
    for a in n.attributes() {
        if rewritten && matches!(a.name(), "timeGap" | "transition" | "transitionDuration") {
            continue;
        }
        if placed.is_some() && matches!(a.name(), "start" | "end" | "startMarker" | "endMarker") {
            continue;
        }
        if tag == "project" && a.name() == "background" {
            continue;
        }
        if a.namespace().is_some() {
            continue;
        }
        push(out, " ")?;
        push(out, a.name())?;
        push(out, "=\"")?;
        if file_attribute(&selection.structure, n, a.name())
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
    if let Some(resolved) = placed {
        push(out, &format!(" start=\"{}\" end=\"{}\"", resolved.local_start, resolved.local_end))?;
    }
    if tag == "project" {
        push(out, " background=\"#00000000\"")?;
    }
    push(out, ">")?;
    for child in n.children() {
        serialize(child, selection, nodes, opts, out, depth.saturating_add(1))?;
    }
    push(out, "</")?;
    push(out, tag)?;
    push(out, ">")?;
    Ok(())
}
