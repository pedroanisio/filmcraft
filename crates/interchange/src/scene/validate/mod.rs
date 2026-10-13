//! Validation of scene documents against the vendored sr-core 1.7.0 schema.
//!
//! Ported from scene-render's `sr-model` (revision c0c668b8, by the same author, borrowed with
//! permission; see `ATTRIBUTION.md`) and adapted to FilmCraft's never-crash rules: no indexing,
//! bounded recursion and suggestion work, and a capped diagnostic count.
//!
//! [`SIMPLE_TYPES`] and [`COMPLEX_TYPES`] (in `tables.rs`) are generated from
//! `schema/sr-core/scene-render.xsd` by the `tables_match_schema` test, which fails when the
//! checked-in file is stale; `FILMCRAFT_BLESS=1` rewrites it. [`structure`] interprets the tables
//! against a parsed document and [`simple`] checks one lexical value against one simple type;
//! `xs:pattern` facets run on the small linear-time engine in [`regex`]. [`rules`] evaluates the
//! Schematron (`scene-render.sch`) natively, as scene-render does.

#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

pub mod regex;
pub mod rules;
pub mod simple;
pub mod structure;

#[cfg(test)]
mod tables_gen;

use roxmltree::Document;

use super::diag::{Collector, Diagnostic};

/// `maxOccurs="unbounded"`.
pub const UNBOUNDED: u32 = u32::MAX;

/// XSD built-in datatypes used by the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    String,
    Double,
    /// `xs:decimal`: kept as its exact text.
    Decimal,
    Boolean,
    Integer,
    Int,
    NonNegativeInteger,
    PositiveInteger,
    UnsignedLong,
    Id,
    IdRef,
    IdRefs,
    NcName,
    NmToken,
    NmTokens,
    AnyUri,
    DateTime,
}

/// Facets of an `xs:restriction`.
#[derive(Debug, Clone, Copy)]
pub struct Restriction {
    /// Index of the base simple type.
    pub base: usize,
    /// Allowed literals; empty when the restriction has no enumeration.
    pub enums: &'static [&'static str],
    pub min_inclusive: Option<f64>,
    pub max_inclusive: Option<f64>,
    pub min_exclusive: Option<f64>,
    pub max_exclusive: Option<f64>,
    /// `xs:pattern` (XSD regular expression, implicitly anchored), exactly as the schema spells it.
    pub pattern: Option<&'static str>,
    /// `xs:maxLength` in characters.
    pub max_length: Option<usize>,
    /// `xs:length`: characters, or the number of items when the base is a list.
    pub length: Option<usize>,
}

/// Variety of a simple type.
#[derive(Debug, Clone, Copy)]
pub enum SimpleKind {
    Builtin(Builtin),
    Restriction(Restriction),
    /// A union of member types (indices).
    Union(&'static [usize]),
    /// A whitespace-separated list of the item type (index).
    List(usize),
}

/// A simple type definition.
#[derive(Debug, Clone, Copy)]
pub struct SimpleType {
    /// XSD name, `xs:<builtin>`, or `owner@attribute` for anonymous types.
    pub name: &'static str,
    pub kind: SimpleKind,
}

/// How an attribute takes part in `xs:ID` / `xs:IDREF` identity constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdKind {
    None,
    Id,
    IdRef,
    IdRefs,
}

/// An attribute declaration.
#[derive(Debug, Clone, Copy)]
pub struct AttrDecl {
    pub name: &'static str,
    /// Index of its simple type.
    pub ty: usize,
    pub required: bool,
    pub default: Option<&'static str>,
    pub id: IdKind,
}

/// A content-model particle; model groups are expanded in place.
#[derive(Debug, Clone, Copy)]
pub enum Particle {
    Element { name: &'static str, ty: usize, min: u32, max: u32 },
    Seq { items: &'static [Particle], min: u32, max: u32 },
    Choice { items: &'static [Particle], min: u32, max: u32 },
}

/// Content of a complex type.
#[derive(Debug, Clone, Copy)]
pub enum Content {
    Empty,
    /// Text of the given simple type.
    Simple(usize),
    /// Element-only content.
    Elements(&'static Particle),
}

/// A complex type definition with extension already applied.
#[derive(Debug, Clone, Copy)]
pub struct ComplexType {
    /// XSD name (`scene` for the anonymous root type).
    pub name: &'static str,
    /// Attribute declarations, attribute groups expanded.
    pub attrs: &'static [AttrDecl],
    pub content: Content,
}

impl ComplexType {
    /// Declaration of attribute `name`.
    pub fn attr(&self, name: &str) -> Option<&'static AttrDecl> {
        self.attrs.iter().find(|a| a.name == name)
    }
}

include!("tables.rs");

/// Root element name.
pub const ROOT_ELEMENT: &str = "scene";

/// Longest restriction/union chain followed through the tables (the schema's are a few deep).
const MAX_TYPE_DEPTH: usize = 64;

pub fn simple_type(ty: usize) -> Option<&'static SimpleType> {
    SIMPLE_TYPES.get(ty)
}

pub fn complex(ty: usize) -> Option<&'static ComplexType> {
    COMPLEX_TYPES.get(ty)
}

/// Complex type by XSD name.
pub fn complex_type(name: &str) -> Option<&'static ComplexType> {
    COMPLEX_TYPES.iter().find(|c| c.name == name)
}

/// Built-in datatype at the root of a simple type's restriction chain, or `None` for unions and lists.
pub fn root_builtin(mut ty: usize) -> Option<Builtin> {
    for _ in 0..MAX_TYPE_DEPTH {
        match simple_type(ty)?.kind {
            SimpleKind::Builtin(b) => return Some(b),
            SimpleKind::Restriction(r) => ty = r.base,
            SimpleKind::Union(_) | SimpleKind::List(_) => return None,
        }
    }
    None
}

/// Whether simple type `ty` is the named type `name` or is derived from it by restriction.
pub fn derives_from(mut ty: usize, name: &str) -> bool {
    for _ in 0..MAX_TYPE_DEPTH {
        let Some(t) = simple_type(ty) else { return false };
        if t.name == name {
            return true;
        }
        match t.kind {
            SimpleKind::Restriction(r) => ty = r.base,
            _ => return false,
        }
    }
    false
}

/// Parses an XSD integer literal (`[+-]?[0-9]+`, at most 38 digits).
pub fn parse_xsd_integer(s: &str) -> Option<i128> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.len() > 38 {
        return None;
    }
    s.strip_prefix('+').unwrap_or(s).parse().ok()
}

/// Parses an XSD 1.0 `xs:double` literal, including `INF`, `-INF` and `NaN`.
pub fn parse_xsd_double(s: &str) -> Option<f64> {
    match s {
        "INF" => return Some(f64::INFINITY),
        "-INF" => return Some(f64::NEG_INFINITY),
        "NaN" => return Some(f64::NAN),
        _ => {}
    }
    let digits = |t: &str| t.bytes().all(|b| b.is_ascii_digit());
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (mantissa, exp) = match body.find(['e', 'E']) {
        Some(i) => (body.get(..i)?, Some(body.get(i + 1..)?)),
        None => (body, None),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (int.is_empty() && frac.is_empty()) || !digits(int) || !digits(frac) {
        return None;
    }
    if let Some(e) = exp {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        if e.is_empty() || !digits(e) {
            return None;
        }
    }
    s.parse().ok()
}

/// Validates a parsed scene document against the schema: its structure and simple types, then
/// the Schematron rules in [`rules`].
///
/// Returns every problem found, in document order; the document is valid when none is an error.
pub fn validate(doc: &Document<'_>) -> Vec<Diagnostic> {
    let mut out = Collector::new(doc);
    structure::validate(doc, &mut out);
    rules::validate(doc, &mut out);
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xsd_doubles() {
        for ok in ["1", "-1.5", ".5", "5.", "1e3", "+2E-2", "1.5e+10"] {
            assert!(parse_xsd_double(ok).is_some(), "{ok}");
        }
        assert_eq!(parse_xsd_double("INF"), Some(f64::INFINITY));
        for bad in ["", "e3", "1e", "inf", "+INF", "1,5", "0x10", ".", "1e3.5", "1.2.3", "-", "infinity", "1e+"] {
            assert!(parse_xsd_double(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn xsd_integers() {
        assert_eq!(parse_xsd_integer("+12"), Some(12));
        assert_eq!(parse_xsd_integer("-0"), Some(0));
        assert!(parse_xsd_integer("1".repeat(39).as_str()).is_none());
        assert!(parse_xsd_integer("1.0").is_none());
    }

    #[test]
    fn tables_are_consistent() {
        // every index in the tables points inside them, so lookups never fall back silently
        for t in SIMPLE_TYPES {
            match t.kind {
                SimpleKind::Restriction(r) => assert!(r.base < SIMPLE_TYPES.len(), "{}", t.name),
                SimpleKind::Union(m) => assert!(m.iter().all(|&i| i < SIMPLE_TYPES.len()), "{}", t.name),
                SimpleKind::List(i) => assert!(i < SIMPLE_TYPES.len(), "{}", t.name),
                SimpleKind::Builtin(_) => {}
            }
            if let SimpleKind::Restriction(Restriction { pattern: Some(p), .. }) = t.kind {
                regex::Regex::new(p).unwrap_or_else(|e| panic!("{}: {p}: {e}", t.name));
            }
        }
        fn particle_ok(p: &Particle) -> bool {
            match p {
                Particle::Element { ty, .. } => *ty < COMPLEX_TYPES.len(),
                Particle::Seq { items, .. } | Particle::Choice { items, .. } => items.iter().all(particle_ok),
            }
        }
        for c in COMPLEX_TYPES {
            assert!(c.attrs.iter().all(|a| a.ty < SIMPLE_TYPES.len()), "{}", c.name);
            match c.content {
                Content::Simple(t) => assert!(t < SIMPLE_TYPES.len()),
                Content::Elements(p) => assert!(particle_ok(p), "{}", c.name),
                Content::Empty => {}
            }
        }
        assert_eq!(complex(ROOT_TYPE).map(|c| c.name), Some(ROOT_ELEMENT));
    }
}
