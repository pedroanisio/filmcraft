//! Diagnostics for scene documents: a stable code, a message, and where the problem is.
//!
//! Codes follow scene-render's (`XML`, `S01`–`S12` structure, `W01`, the Schematron assert ids),
//! so a report reads the same whichever tool produced it. Checks push into a [`Collector`] with a
//! byte offset and the element it concerns; line, column and the element path are worked out once
//! at the end, so a document with many problems costs no more than one with few. The collector
//! keeps at most [`MAX_DIAGNOSTICS`] entries and caps the length of every message.

use std::borrow::Cow;
use std::collections::HashMap;

use roxmltree::{Document, Node, NodeId};
use serde::{Deserialize, Serialize};

/// Most diagnostics one validation keeps; the rest are counted in a final note.
pub const MAX_DIAGNOSTICS: usize = 1000;
/// Longest message or help text kept, in bytes.
pub const MAX_MESSAGE: usize = 512;

/// How serious a diagnostic is. Errors make a document invalid; warnings and notes do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// One problem found in a scene document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable code (`XML`, `S04`, `R12`, …).
    pub code: String,
    pub message: String,
    /// 1-based line and column (in characters) of the problem; 0 when unknown.
    pub line: u32,
    pub column: u32,
    /// Byte offset into the document text.
    pub offset: usize,
    /// XPath-like element path, e.g. `/scene/composition/layer[2]`.
    pub path: String,
    /// A suggestion for fixing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "note",
        };
        write!(f, "{}:{}: {sev}[{}] {} ({})", self.line, self.column, self.code, self.message, self.path)?;
        if let Some(h) = &self.help {
            write!(f, "; {h}")?;
        }
        Ok(())
    }
}

/// Cuts `s` to at most [`MAX_MESSAGE`] bytes at a character boundary.
fn cap(mut s: String) -> String {
    if s.len() > MAX_MESSAGE {
        let mut end = MAX_MESSAGE.saturating_sub(3);
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push_str("...");
    }
    s
}

struct Pending {
    severity: Severity,
    code: Cow<'static, str>,
    message: String,
    offset: usize,
    node: Option<NodeId>,
    help: Option<String>,
}

/// Gathers diagnostics during one validation of `doc`.
pub struct Collector<'a, 'i> {
    doc: &'a Document<'i>,
    pending: Vec<Pending>,
    dropped: usize,
}

impl<'a, 'i> Collector<'a, 'i> {
    pub fn new(doc: &'a Document<'i>) -> Self {
        Collector { doc, pending: Vec::new(), dropped: 0 }
    }

    /// Records a problem at `node` (its start), or at `offset` when that is more precise.
    pub fn push(
        &mut self,
        severity: Severity,
        code: impl Into<Cow<'static, str>>,
        message: String,
        node: Option<Node<'_, '_>>,
        offset: Option<usize>,
        help: Option<String>,
    ) {
        if self.pending.len() >= MAX_DIAGNOSTICS {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let offset = offset.or_else(|| node.map(|n| n.range().start)).unwrap_or(0);
        self.pending.push(Pending { severity, code: code.into(), message: cap(message), offset, node: node.map(|n| n.id()), help: help.map(cap) });
    }

    pub fn error(&mut self, code: impl Into<Cow<'static, str>>, message: String, node: Node<'_, '_>) {
        self.push(Severity::Error, code, message, Some(node), None, None);
    }

    pub fn warn(&mut self, code: impl Into<Cow<'static, str>>, message: String, node: Node<'_, '_>) {
        self.push(Severity::Warning, code, message, Some(node), None, None);
    }

    /// Whether an error has been recorded.
    pub fn has_errors(&self) -> bool {
        self.pending.iter().any(|p| p.severity == Severity::Error)
    }

    /// Resolves positions and paths and returns the diagnostics in document order, without duplicates.
    pub fn finish(self) -> Vec<Diagnostic> {
        let text = self.doc.input_text();
        let lines = LineIndex::new(text);
        let mut paths = Paths::default();
        let mut out: Vec<Diagnostic> = self
            .pending
            .into_iter()
            .map(|p| {
                let (line, column) = lines.locate(text, p.offset);
                let path = match p.node.and_then(|id| self.doc.get_node(id)) {
                    Some(n) => paths.of(n),
                    None => "/".to_string(),
                };
                Diagnostic { severity: p.severity, code: p.code.into_owned(), message: p.message, line, column, offset: p.offset, path, help: p.help }
            })
            .collect();
        out.sort_by(|a, b| (a.offset, a.severity, &a.code, &a.message).cmp(&(b.offset, b.severity, &b.code, &b.message)));
        out.dedup();
        if self.dropped > 0 {
            out.push(Diagnostic {
                severity: Severity::Info,
                code: "LIMIT".to_string(),
                message: format!("{} more diagnostics were not reported (limit {MAX_DIAGNOSTICS})", self.dropped),
                line: 0,
                column: 0,
                offset: text.len(),
                path: "/".to_string(),
                help: None,
            });
        }
        out
    }
}

/// Line starts of a text, for offset → line/column.
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i.saturating_add(1)));
        LineIndex { starts }
    }

    /// 1-based line and column (in characters) of byte `offset`.
    pub fn locate(&self, text: &str, offset: usize) -> (u32, u32) {
        let offset = offset.min(text.len());
        let line = self.starts.partition_point(|&s| s <= offset).max(1);
        let start = self.starts.get(line - 1).copied().unwrap_or(0);
        let column = text.get(start..offset).map_or(0, |s| s.chars().count()).saturating_add(1);
        (u32::try_from(line).unwrap_or(u32::MAX), u32::try_from(column).unwrap_or(u32::MAX))
    }
}

/// Element paths, with each parent's same-name sibling positions computed once.
#[derive(Default)]
struct Paths {
    /// node → (1-based position among same-named siblings, whether it has any)
    position: HashMap<NodeId, (usize, bool)>,
}

impl Paths {
    fn index_children(&mut self, parent: Node<'_, '_>) {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let kids: Vec<Node> = parent.children().filter(Node::is_element).collect();
        for k in &kids {
            *counts.entry(k.tag_name().name()).or_insert(0) += 1;
        }
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for k in &kids {
            let name = k.tag_name().name();
            let n = seen.entry(name).or_insert(0);
            *n += 1;
            let many = counts.get(name).copied().unwrap_or(0) > 1;
            self.position.insert(k.id(), (*n, many));
        }
    }

    fn of(&mut self, n: Node<'_, '_>) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(n).filter(Node::is_element).or_else(|| n.parent_element());
        while let Some(e) = cur {
            let parent = e.parent_element();
            let name = e.tag_name().name();
            match parent {
                Some(p) => {
                    if !self.position.contains_key(&e.id()) {
                        self.index_children(p);
                    }
                    match self.position.get(&e.id()) {
                        Some(&(i, true)) => parts.push(format!("{name}[{i}]")),
                        _ => parts.push(name.to_string()),
                    }
                }
                None => parts.push(name.to_string()),
            }
            cur = parent;
        }
        parts.reverse();
        format!("/{}", parts.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_positions() {
        let xml = "<scene>\n  <a/>\n  <b><c/></b>\n  <b><c/><c/></b>\n</scene>";
        let doc = Document::parse(xml).unwrap();
        let mut col = Collector::new(&doc);
        let last_c = doc.descendants().rfind(|n| n.has_tag_name("c")).unwrap();
        col.error("S01", "x".into(), last_c);
        let a = doc.descendants().find(|n| n.has_tag_name("a")).unwrap();
        col.warn("W01", "y".into(), a);
        let d = col.finish();
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].line, d[0].column, d[0].path.as_str()), (2, 3, "/scene/a"));
        assert_eq!((d[1].line, d[1].path.as_str()), (4, "/scene/b[2]/c[2]"));
    }

    #[test]
    fn caps_count_and_length() {
        let doc = Document::parse("<scene/>").unwrap();
        let mut col = Collector::new(&doc);
        let root = doc.root_element();
        for i in 0..MAX_DIAGNOSTICS + 5 {
            col.error("S01", format!("{i} {}", "é".repeat(400)), root);
        }
        let d = col.finish();
        assert_eq!(d.len(), MAX_DIAGNOSTICS + 1);
        assert!(d.iter().all(|x| x.message.len() <= MAX_MESSAGE));
        assert_eq!(d.last().unwrap().code, "LIMIT");
    }
}
