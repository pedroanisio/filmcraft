//! Bounded reading of an sr-core scene document.
//!
//! [`read`] turns untrusted bytes into a parsed DOM whose size, depth and value lengths are all within
//! [`Limits`], with a `<scene version="1.x">` root in no namespace (the schema has no target namespace
//! and qualified element form). Nothing here interprets the document; validation (S3) and the semantic
//! pass (S4) work on the [`SceneDoc`] this returns.
//!
//! The limits are checked before the DOM exists where possible (input size; nesting depth by a byte scan,
//! because roxmltree recurses once per open element; the node count is enforced by the parser as it
//! allocates) and in one iterative walk afterwards (value lengths, ids), so no input can make the reader
//! recurse deeply, allocate without bound or slice a string mid-character.

use super::err;
use crate::Result;

/// Resource caps for one scene document. Not relaxed by lenient import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Input size in bytes.
    pub max_bytes: usize,
    /// DOM nodes (elements, attributes are not nodes; text, comments and PIs are), enforced by the parser.
    pub max_nodes: u32,
    /// Element nesting depth below the document root (the `<scene>` element is depth 1).
    pub max_depth: usize,
    /// Bytes in one attribute value (after entity expansion).
    pub max_attr_len: usize,
    /// Bytes in one text node.
    pub max_text_len: usize,
    /// Elements carrying an `id` attribute.
    pub max_ids: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_bytes: 64 << 20, max_nodes: 1_000_000, max_depth: 256, max_attr_len: 4 << 20, max_text_len: 16 << 20, max_ids: 500_000 }
    }
}

/// A scene document version (`scene/@version`), 1.0 to 1.7.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneVersion {
    pub major: u8,
    pub minor: u8,
}

impl SceneVersion {
    pub const V1_0: SceneVersion = SceneVersion { major: 1, minor: 0 };
    /// The newest version this reader understands (schema 1.7.0).
    pub const LATEST: SceneVersion = SceneVersion { major: 1, minor: 7 };

    /// Parses `1.0` … `1.7` exactly as the schema enumerates them.
    pub fn parse(s: &str) -> Option<SceneVersion> {
        let (major, minor) = s.split_once('.')?;
        if major != "1" || minor.len() != 1 {
            return None;
        }
        let minor = minor.parse::<u8>().ok().filter(|m| *m <= Self::LATEST.minor)?;
        Some(SceneVersion { major: 1, minor })
    }
}

impl std::fmt::Display for SceneVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// A scene document that passed [`read`].
pub struct SceneDoc<'a> {
    pub doc: roxmltree::Document<'a>,
    pub version: SceneVersion,
}

impl<'a> SceneDoc<'a> {
    /// The `<scene>` element.
    pub fn root(&self) -> roxmltree::Node<'_, 'a> {
        self.doc.root_element()
    }

    /// `line:column` of a node, for diagnostics.
    pub fn position(&self, node: roxmltree::Node<'_, 'a>) -> String {
        let p = self.doc.text_pos_at(node.range().start);
        format!("{}:{}", p.row, p.col)
    }
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Strict UTF-8 text of a scene file: size-capped, BOM removed, UTF-16 refused with a clear message.
pub fn decode<'a>(bytes: &'a [u8], limits: &Limits) -> Result<&'a str> {
    if bytes.len() > limits.max_bytes {
        return Err(err(format!("document is {} bytes; the limit is {}", bytes.len(), limits.max_bytes)));
    }
    if bytes.starts_with(b"\xFF\xFE") || bytes.starts_with(b"\xFE\xFF") {
        return Err(err("document is UTF-16; scene documents are UTF-8"));
    }
    let body = bytes.strip_prefix(BOM).unwrap_or(bytes);
    std::str::from_utf8(body).map_err(|e| err(format!("document is not valid UTF-8 (byte {})", e.valid_up_to())))
}

/// Parses and bounds-checks a scene document.
pub fn read<'a>(text: &'a str, limits: &Limits) -> Result<SceneDoc<'a>> {
    if text.len() > limits.max_bytes {
        return Err(err(format!("document is {} bytes; the limit is {}", text.len(), limits.max_bytes)));
    }
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    check_depth(text, limits.max_depth)?;
    let opts = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: limits.max_nodes, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, opts).map_err(|e| match e {
        roxmltree::Error::NodesLimitReached => err(format!("document has more than {} nodes", limits.max_nodes)),
        e => err(e.to_string()),
    })?;

    let root = doc.root_element();
    if root.tag_name().name() != "scene" {
        return Err(err(format!("root element is <{}>, not <scene>", root.tag_name().name())));
    }
    if let Some(ns) = root.tag_name().namespace() {
        return Err(err(format!("<scene> is in namespace {ns:?}; scene documents use no namespace")));
    }
    let version = match root.attribute("version") {
        None => return Err(err("<scene> has no version attribute")),
        Some(v) => {
            SceneVersion::parse(v).ok_or_else(|| err(format!("unsupported scene version {v:?} (this reader handles 1.0 to {})", SceneVersion::LATEST)))?
        }
    };

    check_shape(&doc, limits)?;
    Ok(SceneDoc { doc, version })
}

/// Refuses element nesting deeper than `max_depth` (the root element is depth 1) before the recursive
/// parser sees it. Skips comments, CDATA, processing instructions, declarations and quoted attribute
/// values, so the count is exact for well-formed XML; anything malformed is the parser's to refuse.
fn check_depth(text: &str, max_depth: usize) -> Result<()> {
    let b = text.as_bytes();
    let skip_past = |from: usize, end: &[u8]| find(b.get(from..).unwrap_or(&[]), end).map_or(b.len(), |i| from + i + end.len());
    let (mut i, mut depth) = (0usize, 0usize);
    while let Some(off) = b.get(i..).and_then(|r| r.iter().position(|&c| c == b'<')) {
        let lt = i + off;
        let rest = b.get(lt..).unwrap_or(&[]);
        if rest.starts_with(b"<!--") {
            i = skip_past(lt + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = skip_past(lt + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = skip_past(lt + 2, b"?>");
        } else if rest.starts_with(b"<!") {
            // a DTD is refused by the parser; its markup does not nest elements
            i = skip_past(lt + 2, b">");
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = skip_past(lt + 2, b">");
        } else {
            let (mut j, mut quote, mut empty) = (lt + 1, None, false);
            while let Some(&c) = b.get(j) {
                j += 1;
                match (quote, c) {
                    (Some(q), _) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(c),
                    (None, b'>') => {
                        empty = b.get(j.saturating_sub(2)) == Some(&b'/');
                        break;
                    }
                    (None, _) => {}
                }
            }
            // an empty element sits one level down too, it just does not stay open
            let d = depth.saturating_add(1);
            if d > max_depth {
                let head = text.get(..lt).unwrap_or("");
                let row = head.matches('\n').count() + 1;
                let col = head.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
                return Err(err(format!("elements nest deeper than {max_depth} at {row}:{col}")));
            }
            if !empty {
                depth = d;
            }
            i = j;
        }
    }
    Ok(())
}

/// One pass over every node in document order: value lengths, id count.
fn check_shape(doc: &roxmltree::Document<'_>, limits: &Limits) -> Result<()> {
    let at = |node: roxmltree::Node<'_, '_>| {
        let p = doc.text_pos_at(node.range().start);
        format!("{}:{}", p.row, p.col)
    };
    let mut ids = 0usize;
    for node in doc.descendants() {
        if node.is_element() {
            for a in node.attributes() {
                if a.value().len() > limits.max_attr_len {
                    return Err(err(format!("attribute {} at {} is longer than {} bytes", a.name(), at(node), limits.max_attr_len)));
                }
            }
            if node.has_attribute("id") {
                ids += 1;
                if ids > limits.max_ids {
                    return Err(err(format!("document has more than {} ids", limits.max_ids)));
                }
            }
        } else if let Some(t) = node.text()
            && t.len() > limits.max_text_len
        {
            return Err(err(format!("text at {} is longer than {} bytes", at(node), limits.max_text_len)));
        }
    }
    Ok(())
}

/// True when `bytes` look like a scene document: the first element is `<scene`. Reads at most 4 KiB
/// and skips a BOM, the XML declaration, comments, processing instructions and whitespace.
pub fn sniff(bytes: &[u8]) -> bool {
    let head = bytes.get(..bytes.len().min(4096)).unwrap_or(&[]);
    let mut s = head.strip_prefix(BOM).unwrap_or(head);
    loop {
        s = s.trim_ascii_start();
        if let Some(rest) = s.strip_prefix(b"<!--") {
            match find(rest, b"-->") {
                Some(i) => s = rest.get(i + 3..).unwrap_or(&[]),
                None => return false,
            }
        } else if let Some(rest) = s.strip_prefix(b"<?") {
            match find(rest, b"?>") {
                Some(i) => s = rest.get(i + 2..).unwrap_or(&[]),
                None => return false,
            }
        } else {
            return s.strip_prefix(b"<scene").and_then(|r| r.first()).is_some_and(|c| c.is_ascii_whitespace() || matches!(c, b'>' | b'/'));
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use crate::scene::FORMAT;

    const MIN: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<scene version="1.7">
  <project width="1920" height="1080" fps="30" duration="5"/>
  <composition/>
</scene>"##;

    fn message(r: Result<SceneDoc<'_>>) -> String {
        match r {
            Err(Error::Parse { format, message }) => {
                assert_eq!(format, FORMAT);
                message
            }
            Err(e) => panic!("unexpected error {e}"),
            Ok(_) => panic!("accepted"),
        }
    }

    #[test]
    fn reads_minimal_document() {
        let d = read(MIN, &Limits::default()).unwrap();
        assert_eq!(d.version, SceneVersion::LATEST);
        assert_eq!(d.root().tag_name().name(), "scene");
        let project = d.root().first_element_child().unwrap();
        assert_eq!(d.position(project), "3:3");
    }

    #[test]
    fn versions() {
        for (v, ok) in
            [("1.0", true), ("1.7", true), ("1.8", false), ("2.0", false), ("1.07", false), ("1", false), ("", false), ("1.-1", false), (" 1.0", false)]
        {
            assert_eq!(SceneVersion::parse(v).is_some(), ok, "{v}");
        }
        assert!(message(read(r#"<scene><project/></scene>"#, &Limits::default())).contains("no version"));
        assert!(message(read(r#"<scene version="1.9"/>"#, &Limits::default())).contains("unsupported"));
    }

    #[test]
    fn root_must_be_scene_without_namespace() {
        assert!(message(read(r#"<fcpxml version="1.10"/>"#, &Limits::default())).contains("<fcpxml>"));
        assert!(message(read(r#"<scene xmlns="urn:x" version="1.0"/>"#, &Limits::default())).contains("namespace"));
    }

    #[test]
    fn decode_is_strict() {
        let l = Limits::default();
        assert_eq!(decode(b"\xEF\xBB\xBF<scene/>", &l).ok(), Some("<scene/>"));
        assert!(decode(b"<scene>\xFF</scene>", &l).is_err());
        assert!(decode(b"\xFF\xFE<\0s\0", &l).is_err());
        let small = Limits { max_bytes: 4, ..l };
        assert!(decode(b"<scene/>", &small).is_err());
    }

    #[test]
    fn dtd_and_entities_are_refused() {
        let laughs =
            r#"<?xml version="1.0"?><!DOCTYPE scene [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">]><scene version="1.0">&b;</scene>"#;
        assert!(read(laughs, &Limits::default()).is_err());
        let external = r#"<!DOCTYPE scene SYSTEM "file:///etc/passwd"><scene version="1.0"/>"#;
        assert!(read(external, &Limits::default()).is_err());
    }

    #[test]
    fn hostile_depth_is_refused_without_recursion() {
        let n = 100_000;
        let doc = format!(r#"<scene version="1.0">{}{}</scene>"#, "<group>".repeat(n), "</group>".repeat(n));
        let l = Limits { max_bytes: usize::MAX, ..Limits::default() };
        assert!(message(read(&doc, &l)).contains("nest deeper"));
        let ok = format!(r#"<scene version="1.0">{}{}</scene>"#, "<g>".repeat(255), "</g>".repeat(255));
        assert!(read(&ok, &l).is_ok());
        let deep = format!(r#"<scene version="1.0">{}{}</scene>"#, "<g>".repeat(256), "</g>".repeat(256));
        assert!(message(read(&deep, &l)).contains("nest deeper than 256 at 1:"));
    }

    #[test]
    fn depth_scan_skips_markup_that_does_not_nest() {
        let l = Limits { max_depth: 3, ..Limits::default() };
        let flat = r#"<?xml version="1.0"?><!-- <a><b><c> --><scene version="1.0"><a t="x>y" u='<'/><b><![CDATA[<c><d><e>]]><?pi <f><g>?></b><c/></scene>"#;
        // `<` in an attribute value is not well-formed; the parser, not the depth scan, refuses it
        assert!(!message(read(flat, &l)).contains("nest"));
        let flat = flat.replace(" u='<'", " u='&gt;'");
        assert!(read(&flat, &l).is_ok());
        let deep = r#"<scene version="1.0">
  <a><b><c/></b></a>
</scene>"#;
        assert!(message(read(deep, &l)).contains("nest deeper than 3 at 2:9"));
    }

    #[test]
    fn hostile_sizes_are_refused() {
        let l = Limits { max_nodes: 100, ..Limits::default() };
        let doc = format!(r#"<scene version="1.0">{}</scene>"#, "<a/>".repeat(200));
        assert!(message(read(&doc, &l)).contains("more than 100 nodes"));

        let l = Limits { max_attr_len: 8, ..Limits::default() };
        assert!(message(read(r#"<scene version="1.0"><a d="0123456789"/></scene>"#, &l)).contains("attribute d"));

        let l = Limits { max_text_len: 8, ..Limits::default() };
        assert!(message(read(r#"<scene version="1.0"><a>0123456789</a></scene>"#, &l)).contains("text at"));

        let l = Limits { max_ids: 2, ..Limits::default() };
        assert!(message(read(r#"<scene version="1.0"><a id="1"/><a id="2"/><a id="3"/></scene>"#, &l)).contains("ids"));

        let l = Limits { max_bytes: 10, ..Limits::default() };
        assert!(message(read(MIN, &l)).contains("bytes"));
    }

    #[test]
    fn malformed_xml_is_an_error() {
        for bad in [
            "",
            "<",
            "<scene version=\"1.0\">",
            "<scene version=\"1.0\"></scen>",
            "\u{FEFF}",
            "<scene version=\"1.0\"/><scene version=\"1.0\"/>",
            "<scene version='1.0' version='1.0'/>",
        ] {
            assert!(read(bad, &Limits::default()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn sniffing() {
        assert!(sniff(MIN.as_bytes()));
        assert!(sniff(b"\xEF\xBB\xBF<!-- c < d --><?pi x?>\n<scene version=\"1.0\"/>"));
        assert!(!sniff(b"<scenery/>"));
        assert!(!sniff(b"<fcpxml version=\"1.10\">"));
        assert!(!sniff(b"<!-- unterminated <scene"));
        assert!(!sniff(b""));
        let mut padded = vec![b' '; 5000];
        padded.extend_from_slice(b"<scene version=\"1.0\"/>");
        assert!(!sniff(&padded));
    }
}
