//! Structural validation: content models, attributes, simple values, IDs.
//!
//! Diagnostic codes (scene-render's):
//!
//! | code | meaning |
//! |------|---------|
//! | S01 | root element is not `<scene>` |
//! | S02 | element not allowed here (unknown, misplaced or too many) |
//! | S03 | required child element missing |
//! | S04 | attribute not declared for this element |
//! | S05 | required attribute missing |
//! | S06 | attribute value violates its type |
//! | S07 | text inside element-only content |
//! | S08 | text content violates its type |
//! | S09 | duplicate `xs:ID` value |
//! | S10 | `xs:IDREF` value names no `xs:ID` |
//! | S11 | element or attribute in a foreign namespace |
//! | S12 | nesting deeper than the supported limit |
//! | W01 | non-finite number (`INF`, `-INF`, `NaN` or an overflowing literal) in a numeric attribute |
//!
//! Adapted from scene-render `crates/sr-model/src/xsd/structure.rs` (revision c0c668b8,
//! © Pedro Anisio Silva), relicensed by its author for FilmCraft under MIT OR Apache-2.0; see
//! `ATTRIBUTION.md`.

use std::collections::{BTreeSet, HashMap};

use roxmltree::{Document, Node, NodeId};

use super::simple::{self, collapse};
use super::{ComplexType, Content, IdKind, Particle, ROOT_ELEMENT, ROOT_TYPE, complex};
use crate::scene::diag::{Collector, LineIndex, Severity};

const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// Maximum element nesting depth accepted.
pub const MAX_DEPTH: usize = 256;

/// Maximum structural errors reported for the children of one element.
const MAX_CHILD_ERRORS: usize = 32;

/// Words longer than this get no "did you mean" suggestion (the edit distance is quadratic).
const MAX_SUGGEST_LEN: usize = 64;
/// Unresolved references that get a suggestion; the rest are reported without one.
const MAX_REF_SUGGESTIONS: usize = 32;
/// Documents with more ids than this get no suggestions for unresolved references.
const MAX_SUGGEST_IDS: usize = 10_000;

/// What the structural pass learned about a document, for the later passes.
#[derive(Debug, Default)]
pub struct Structure {
    /// Every `xs:ID` value with the element that carries it (the first, when duplicated).
    pub ids: HashMap<String, NodeId>,
    /// The complex type each element was validated against: the declarations (and so the attribute
    /// defaults) that apply to it. Elements the content model rejected have none.
    pub types: HashMap<NodeId, &'static ComplexType>,
}

struct IdRefUse<'a, 'i> {
    value: String,
    node: Node<'a, 'i>,
    attr: &'static str,
}

struct Walker<'o, 'c, 'a, 'i> {
    out: &'o mut Collector<'c, 'i>,
    text: &'i str,
    lines: Option<LineIndex>,
    ids: HashMap<String, Node<'a, 'i>>,
    refs: Vec<IdRefUse<'a, 'i>>,
    types: HashMap<NodeId, &'static ComplexType>,
}

/// Validates the document structure against the schema, pushing diagnostics into `out`.
pub fn validate<'i>(doc: &Document<'i>, out: &mut Collector<'_, 'i>) -> Structure {
    let root = doc.root_element();
    if root.tag_name().name() != ROOT_ELEMENT || root.tag_name().namespace().is_some() {
        out.push(
            Severity::Error,
            "S01",
            format!("root element is <{}>; a scene-render document starts with <scene>", qname(root)),
            Some(root),
            None,
            Some("wrap the document in <scene version=\"1.7\"> … </scene>".into()),
        );
        return Structure::default();
    }
    let Some(root_type) = complex(ROOT_TYPE) else { return Structure::default() };
    let mut w = Walker { out, text: doc.input_text(), lines: None, ids: HashMap::new(), refs: Vec::new(), types: HashMap::new() };
    w.element(root, root_type, 0);
    w.check_refs();
    Structure { ids: w.ids.into_iter().map(|(k, n)| (k, n.id())).collect(), types: w.types }
}

fn qname(n: Node) -> String {
    match n.tag_name().namespace() {
        Some(ns) => format!("{{{ns}}}{}", n.tag_name().name()),
        None => n.tag_name().name().to_string(),
    }
}

/// Levenshtein distance, ignoring ASCII case, for "did you mean" suggestions.
pub(crate) fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = Vec::with_capacity(b.len().saturating_add(1));
        cur.push(i.saturating_add(1));
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(!ca.eq_ignore_ascii_case(cb));
            let del = prev.get(j.saturating_add(1)).copied().unwrap_or(usize::MAX).saturating_add(1);
            let ins = cur.last().copied().unwrap_or(usize::MAX).saturating_add(1);
            let sub = prev.get(j).copied().unwrap_or(usize::MAX).saturating_add(cost);
            cur.push(del.min(ins).min(sub));
        }
        prev = cur;
    }
    prev.last().copied().unwrap_or(0)
}

/// The closest candidate within a small edit distance.
pub(crate) fn suggest<'c>(word: &str, candidates: impl IntoIterator<Item = &'c str>) -> Option<&'c str> {
    let len = word.chars().count();
    if len > MAX_SUGGEST_LEN {
        return None;
    }
    let limit = (len / 3).clamp(1, 3);
    candidates
        .into_iter()
        .filter(|c| c.len() <= MAX_SUGGEST_LEN.saturating_mul(4))
        .map(|c| (distance(word, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, c)| (*d, c.len()))
        .map(|(_, c)| c)
}

fn is_ws(s: &str) -> bool {
    s.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
}

fn format_names(set: &BTreeSet<&'static str>) -> String {
    let v: Vec<String> = set.iter().map(|n| format!("<{n}>")).collect();
    match v.as_slice() {
        [] => "no child elements".to_string(),
        [one] => one.clone(),
        _ => format!("one of {}", v.join(", ")),
    }
}

// ------------------------------------------------------------------ particle matching

struct Missing {
    pos: usize,
    names: BTreeSet<&'static str>,
}

/// Greedy content-model matcher. A sequence repetition that fails is rolled back through an undo
/// trail of assignments, which reproduces scene-render's clone-and-restore without its quadratic cost.
struct Matcher<'k, 'a, 'i> {
    kids: &'k [Node<'a, 'i>],
    /// Complex type index assigned to each child.
    assign: Vec<Option<usize>>,
    /// `(position, previous assignment)` for every assignment, newest last.
    trail: Vec<(usize, Option<usize>)>,
    /// Element names that could have appeared at each position.
    expect: Vec<BTreeSet<&'static str>>,
}

fn first_names(p: &Particle, out: &mut BTreeSet<&'static str>) {
    match p {
        Particle::Element { name, .. } => {
            out.insert(*name);
        }
        Particle::Choice { items, .. } => items.iter().for_each(|i| first_names(i, out)),
        Particle::Seq { items, .. } => {
            for i in items.iter() {
                first_names(i, out);
                if min_of(i) > 0 {
                    break;
                }
            }
        }
    }
}

fn min_of(p: &Particle) -> u32 {
    match p {
        Particle::Element { min, .. } | Particle::Seq { min, .. } | Particle::Choice { min, .. } => *min,
    }
}

impl<'k, 'a, 'i> Matcher<'k, 'a, 'i> {
    fn new(kids: &'k [Node<'a, 'i>]) -> Self {
        Matcher { kids, assign: vec![None; kids.len()], trail: Vec::new(), expect: vec![BTreeSet::new(); kids.len().saturating_add(1)] }
    }

    fn is(&self, pos: usize, name: &str) -> bool {
        self.kids.get(pos).is_some_and(|k| k.tag_name().name() == name && k.tag_name().namespace().is_none())
    }

    fn set(&mut self, pos: usize, ty: usize) {
        if let Some(slot) = self.assign.get_mut(pos) {
            self.trail.push((pos, *slot));
            *slot = Some(ty);
        }
    }

    fn undo(&mut self, mark: usize) {
        while self.trail.len() > mark {
            let Some((pos, old)) = self.trail.pop() else { break };
            if let Some(slot) = self.assign.get_mut(pos) {
                *slot = old;
            }
        }
    }

    fn m(&mut self, p: &Particle, mut pos: usize) -> Result<usize, Missing> {
        match p {
            Particle::Element { name, ty, min, max } => {
                let mut count = 0;
                while count < *max && self.is(pos, name) {
                    self.set(pos, *ty);
                    pos = pos.saturating_add(1);
                    count += 1;
                }
                if count < *max
                    && let Some(e) = self.expect.get_mut(pos)
                {
                    e.insert(*name);
                }
                if count < *min {
                    return Err(Missing { pos, names: BTreeSet::from([*name]) });
                }
                Ok(pos)
            }
            Particle::Seq { items, min, max } => {
                let mut count = 0;
                while count < *max {
                    let start = pos;
                    let mark = self.trail.len();
                    let mut cur = pos;
                    let mut failed = None;
                    for it in items.iter() {
                        match self.m(it, cur) {
                            Ok(np) => cur = np,
                            Err(e) => {
                                failed = Some(e);
                                break;
                            }
                        }
                    }
                    match failed {
                        Some(e) if count < *min => return Err(e),
                        Some(_) => {
                            self.undo(mark);
                            break;
                        }
                        None => {
                            pos = cur;
                            count += 1;
                            if pos == start {
                                break;
                            }
                        }
                    }
                }
                if count < *min {
                    let mut names = BTreeSet::new();
                    first_names(p, &mut names);
                    return Err(Missing { pos, names });
                }
                Ok(pos)
            }
            Particle::Choice { items, min, max } => {
                let mut count = 0;
                while count < *max {
                    let mut progressed = false;
                    for it in items.iter() {
                        if let Ok(np) = self.m(it, pos)
                            && np > pos
                        {
                            pos = np;
                            progressed = true;
                            break;
                        }
                    }
                    if !progressed {
                        break;
                    }
                    count += 1;
                }
                if count < *min {
                    let mut names = BTreeSet::new();
                    first_names(p, &mut names);
                    return Err(Missing { pos, names });
                }
                Ok(pos)
            }
        }
    }
}

fn allowed_children(p: &Particle) -> BTreeSet<&'static str> {
    fn walk(p: &Particle, out: &mut BTreeSet<&'static str>) {
        match p {
            Particle::Element { name, .. } => {
                out.insert(*name);
            }
            Particle::Seq { items, .. } | Particle::Choice { items, .. } => items.iter().for_each(|i| walk(i, out)),
        }
    }
    let mut s = BTreeSet::new();
    walk(p, &mut s);
    s
}

// ------------------------------------------------------------------ walker

impl<'a, 'i> Walker<'_, '_, 'a, 'i> {
    fn error(&mut self, code: &'static str, message: String, node: Node<'a, 'i>, offset: Option<usize>) {
        self.out.push(Severity::Error, code, message, Some(node), offset, None);
    }

    fn line_of(&mut self, offset: usize) -> u32 {
        let text = self.text;
        self.lines.get_or_insert_with(|| LineIndex::new(text)).locate(text, offset).0
    }

    fn element(&mut self, n: Node<'a, 'i>, ct: &'static ComplexType, depth: usize) {
        if depth > MAX_DEPTH {
            self.error("S12", format!("elements are nested more than {MAX_DEPTH} levels deep"), n, None);
            return;
        }
        self.types.insert(n.id(), ct);
        self.attributes(n, ct);
        let ename = n.tag_name().name();
        match ct.content {
            Content::Empty => {
                for c in n.children() {
                    if c.is_element() {
                        let msg = format!("<{}> is not allowed inside <{ename}>, which takes no child elements", qname(c));
                        self.error("S02", msg, c, None);
                    } else if c.is_text() && !is_ws(c.text().unwrap_or("")) {
                        self.text_error(n, c);
                    }
                }
            }
            Content::Simple(ty) => {
                let mut text = String::new();
                for c in n.children() {
                    if c.is_element() {
                        let msg = format!("<{}> is not allowed inside <{ename}>, which takes text only", qname(c));
                        self.error("S02", msg, c, None);
                    } else if c.is_text() {
                        text.push_str(c.text().unwrap_or(""));
                    }
                }
                if let Err(e) = simple::check(ty, &text) {
                    self.error("S08", format!("text of <{ename}>: {e}"), n, None);
                }
            }
            Content::Elements(p) => self.children(n, p, depth),
        }
    }

    fn validate_assigned(&mut self, kids: &[Node<'a, 'i>], assign: Vec<Option<usize>>, depth: usize) {
        for (k, t) in kids.iter().zip(assign) {
            if let Some(ct) = t.and_then(complex) {
                self.element(*k, ct, depth.saturating_add(1));
            }
        }
    }

    fn children(&mut self, n: Node<'a, 'i>, p: &'static Particle, depth: usize) {
        let ename = n.tag_name().name();
        for c in n.children() {
            if c.is_text() && !is_ws(c.text().unwrap_or("")) {
                self.text_error(n, c);
            }
        }
        let allowed = allowed_children(p);
        let mut kids: Vec<Node<'a, 'i>> = n.children().filter(|c| c.is_element()).collect();
        let mut errors = 0;
        loop {
            let mut m = Matcher::new(&kids);
            let res = m.m(p, 0);
            let (bad, expected) = match res {
                Ok(pos) if pos == kids.len() => {
                    self.validate_assigned(&kids, m.assign, depth);
                    return;
                }
                Ok(pos) => (pos, m.expect.get_mut(pos).map(std::mem::take).unwrap_or_default()),
                Err(miss) if kids.get(miss.pos).is_some_and(|k| !miss.names.contains(k.tag_name().name())) => {
                    let mut exp = m.expect.get_mut(miss.pos).map(std::mem::take).unwrap_or_default();
                    exp.extend(miss.names);
                    (miss.pos, exp)
                }
                Err(miss) => {
                    let at = kids.get(miss.pos).copied();
                    let msg = match at {
                        Some(k) => format!("<{ename}> is missing {} before <{}>", format_names(&miss.names), k.tag_name().name()),
                        None => format!("<{ename}> is missing required child {}", format_names(&miss.names)),
                    };
                    self.error("S03", msg, at.unwrap_or(n), None);
                    self.validate_assigned(&kids, m.assign, depth);
                    return;
                }
            };
            let Some(&k) = kids.get(bad) else { return };
            let name = k.tag_name().name();
            let (code, msg) = match k.tag_name().namespace() {
                Some(ns) => ("S11", format!("<{name}> is in namespace {ns}; scene-render elements have no namespace")),
                None => {
                    let reason = if allowed.contains(name) { "is out of order or repeated too often" } else { "is not allowed" };
                    ("S02", format!("<{name}> {reason} inside <{ename}>; expected {}", format_names(&expected)))
                }
            };
            let help = if allowed.contains(name) { None } else { suggest(name, allowed.iter().copied()).map(|s| format!("did you mean <{s}>?")) };
            self.out.push(Severity::Error, code, msg, Some(k), None, help);
            kids.remove(bad);
            errors += 1;
            if errors >= MAX_CHILD_ERRORS {
                return;
            }
        }
    }

    fn text_error(&mut self, parent: Node<'a, 'i>, text: Node<'a, 'i>) {
        let raw = text.text().unwrap_or("");
        let shown: String = raw.trim().chars().take(40).collect();
        let lead = raw.find(|c: char| !c.is_whitespace()).unwrap_or(0);
        let msg = format!("text {shown:?} is not allowed inside <{}>", parent.tag_name().name());
        self.error("S07", msg, parent, Some(text.range().start.saturating_add(lead)));
    }

    fn attributes(&mut self, n: Node<'a, 'i>, ct: &'static ComplexType) {
        let ename = n.tag_name().name();
        for a in n.attributes() {
            if a.namespace() == Some(XSI) {
                continue;
            }
            let at = Some(a.range().start);
            if let Some(ns) = a.namespace() {
                let msg = format!("attribute {ns}:{} is in namespace {ns}, which <{ename}> does not accept", a.name());
                self.error("S11", msg, n, at);
                continue;
            }
            let Some(decl) = ct.attr(a.name()) else {
                let help = suggest(a.name(), ct.attrs.iter().map(|d| d.name)).map(|s| format!("did you mean @{s}?"));
                let msg = format!("<{ename}> has no attribute @{}", a.name());
                self.out.push(Severity::Error, "S04", msg, Some(n), at, help);
                continue;
            };
            let value = a.value();
            if let Err(e) = simple::check(decl.ty, value) {
                self.error("S06", format!("@{} of <{ename}>: {e}", decl.name), n, at);
                continue;
            }
            if let Some(v) = simple::non_finite(decl.ty, value) {
                let msg = format!("@{} of <{ename}> is {v}; the renderer requires finite numbers here", decl.name);
                self.out.push(Severity::Warning, "W01", msg, Some(n), at, None);
            }
            match decl.id {
                IdKind::None => {}
                IdKind::Id => {
                    let v = collapse(value);
                    if let Some(&first) = self.ids.get(&v) {
                        let line = self.line_of(first.range().start);
                        let msg = format!("ID {v:?} is already used by <{}> at line {line}", first.tag_name().name());
                        let help = Some("every id must be unique across the whole document".to_string());
                        self.out.push(Severity::Error, "S09", msg, Some(n), at, help);
                    } else {
                        self.ids.insert(v, n);
                    }
                }
                IdKind::IdRef => self.refs.push(IdRefUse { value: collapse(value), node: n, attr: decl.name }),
                IdKind::IdRefs => {
                    for t in collapse(value).split(' ') {
                        self.refs.push(IdRefUse { value: t.to_string(), node: n, attr: decl.name });
                    }
                }
            }
        }
        for decl in ct.attrs.iter().filter(|d| d.required) {
            if n.attribute(decl.name).is_none() {
                self.error("S05", format!("<{ename}> requires @{}", decl.name), n, None);
            }
        }
    }

    fn check_refs(&mut self) {
        let mut suggestions = if self.ids.len() <= MAX_SUGGEST_IDS { MAX_REF_SUGGESTIONS } else { 0 };
        for r in std::mem::take(&mut self.refs) {
            if self.ids.contains_key(&r.value) {
                continue;
            }
            let at = r.node.attributes().find(|a| a.name() == r.attr && a.namespace().is_none()).map_or_else(|| r.node.range().start, |a| a.range().start);
            let msg = format!("@{} of <{}> refers to {:?}, but no element has that id", r.attr, r.node.tag_name().name(), r.value);
            let help = if suggestions > 0 {
                suggestions -= 1;
                suggest(&r.value, self.ids.keys().map(String::as_str)).map(|s| format!("did you mean {s:?}?"))
            } else {
                None
            };
            self.out.push(Severity::Error, "S10", msg, Some(r.node), Some(at), help);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance() {
        assert_eq!(distance("layer", "layer"), 0);
        assert_eq!(distance("layr", "layer"), 1);
        assert_eq!(distance("Layer", "layer"), 0);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(suggest("layr", ["layer", "group"]), Some("layer"));
        assert_eq!(suggest(&"x".repeat(100), ["x"]), None);
    }

    #[test]
    fn undo_trail_restores_assignments() {
        let doc = Document::parse("<p><a/><b/></p>").unwrap();
        let kids: Vec<Node> = doc.root_element().children().filter(Node::is_element).collect();
        let mut m = Matcher::new(&kids);
        let mark = m.trail.len();
        m.set(0, 7);
        m.set(0, 8);
        m.set(1, 9);
        m.undo(mark);
        assert_eq!(m.assign, vec![None, None]);
    }
}
