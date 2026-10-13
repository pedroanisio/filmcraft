//! Generates `tables.rs` from the vendored XSD and checks the checked-in copy is current.
//!
//! A test rather than a build script, so building the crate never parses the schema: the tables are
//! plain checked-in source. Ported from sr-model's `build.rs` (parse_schema, expand_particle,
//! emit_tables). It supports exactly the XSD subset the schema uses and panics on anything else, so
//! a schema update that needs more support fails here instead of being silently ignored.
//!
//! Adapted from scene-render `crates/sr-model/build.rs` (revision c0c668b8,
//! © Pedro Anisio Silva), relicensed by its author for FilmCraft under MIT OR Apache-2.0; see
//! `ATTRIBUTION.md`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;

const XS: &str = "http://www.w3.org/2001/XMLSchema";
const UNBOUNDED: u32 = u32::MAX;

const BUILTINS: &[(&str, &str)] = &[
    ("string", "String"),
    ("double", "Double"),
    ("decimal", "Decimal"),
    ("boolean", "Boolean"),
    ("integer", "Integer"),
    ("int", "Int"),
    ("nonNegativeInteger", "NonNegativeInteger"),
    ("positiveInteger", "PositiveInteger"),
    ("unsignedLong", "UnsignedLong"),
    ("ID", "Id"),
    ("IDREF", "IdRef"),
    ("IDREFS", "IdRefs"),
    ("NCName", "NcName"),
    ("NMTOKEN", "NmToken"),
    ("NMTOKENS", "NmTokens"),
    ("anyURI", "AnyUri"),
    ("dateTime", "DateTime"),
];

#[derive(Clone, Debug)]
enum SKind {
    Builtin(&'static str),
    Restr {
        base: usize,
        enums: Vec<String>,
        min_incl: Option<f64>,
        max_incl: Option<f64>,
        min_excl: Option<f64>,
        max_excl: Option<f64>,
        pattern: Option<String>,
        max_len: Option<usize>,
        len: Option<usize>,
    },
    Union(Vec<usize>),
    List(usize),
}

#[derive(Clone, Debug)]
struct SDef {
    name: String,
    kind: SKind,
}

#[derive(Clone, Debug)]
struct Attr {
    name: String,
    ty: usize,
    required: bool,
    default: Option<String>,
}

#[derive(Clone, Debug)]
enum Particle {
    Element { name: String, ty: String, min: u32, max: u32 },
    Seq { items: Vec<Particle>, min: u32, max: u32 },
    Choice { items: Vec<Particle>, min: u32, max: u32 },
    Group { name: String, min: u32, max: u32 },
}

#[derive(Clone, Debug)]
enum Content {
    Empty,
    Simple(usize),
    Elements(Particle),
}

#[derive(Clone, Debug)]
struct CDef {
    name: String,
    attrs: Vec<Attr>,
    content: Content,
    base: Option<String>,
}

struct Schema {
    simple: Vec<SDef>,
    simple_by_name: HashMap<String, usize>,
    complex: Vec<CDef>,
    complex_by_name: HashMap<String, usize>,
    groups: BTreeMap<String, Particle>,
}

type N<'a, 'i> = roxmltree::Node<'a, 'i>;

fn xs_children<'a, 'i>(n: N<'a, 'i>) -> impl Iterator<Item = N<'a, 'i>> {
    n.children().filter(|c| c.is_element() && c.tag_name().namespace() == Some(XS))
}

fn local<'a>(n: N<'a, '_>) -> &'a str {
    n.tag_name().name()
}

fn occurs(n: N) -> (u32, u32) {
    let min = n.attribute("minOccurs").map(|v| v.parse().unwrap()).unwrap_or(1);
    let max = match n.attribute("maxOccurs") {
        Some("unbounded") => UNBOUNDED,
        Some(v) => v.parse().unwrap(),
        None => 1,
    };
    (min, max)
}

struct Parser<'a, 'i> {
    named_simple: HashMap<String, N<'a, 'i>>,
    attr_groups: HashMap<String, N<'a, 'i>>,
    s: Schema,
}

impl<'a, 'i> Parser<'a, 'i> {
    fn builtin(&mut self, local_name: &str) -> usize {
        let key = format!("xs:{local_name}");
        if let Some(&i) = self.s.simple_by_name.get(&key) {
            return i;
        }
        let variant = BUILTINS.iter().find(|(n, _)| *n == local_name).unwrap_or_else(|| panic!("unsupported builtin xs:{local_name}")).1;
        let i = self.s.simple.len();
        self.s.simple.push(SDef { name: key.clone(), kind: SKind::Builtin(variant) });
        self.s.simple_by_name.insert(key, i);
        i
    }

    fn type_ref(&mut self, qname: &str) -> usize {
        if let Some(l) = qname.strip_prefix("xs:") {
            return self.builtin(l);
        }
        if let Some(&i) = self.s.simple_by_name.get(qname) {
            return i;
        }
        let node = *self.named_simple.get(qname).unwrap_or_else(|| panic!("unknown simple type {qname}"));
        self.simple_type(node, qname.to_string())
    }

    fn simple_type(&mut self, n: N<'a, 'i>, name: String) -> usize {
        let body = xs_children(n).find(|c| local(*c) != "annotation").expect("empty simpleType");
        let kind = match local(body) {
            "restriction" => {
                let base = match body.attribute("base") {
                    Some(b) => self.type_ref(b),
                    None => {
                        let inner = xs_children(body).find(|c| local(*c) == "simpleType").expect("restriction base or simpleType");
                        self.simple_type(inner, format!("{name}.base"))
                    }
                };
                let mut enums = Vec::new();
                let (mut min_incl, mut max_incl, mut min_excl, mut max_excl) = (None, None, None, None);
                let mut pattern = None;
                let mut max_len = None;
                let mut len = None;
                for f in xs_children(body) {
                    let v = f.attribute("value").unwrap_or_default();
                    match local(f) {
                        "enumeration" => enums.push(v.to_string()),
                        "minInclusive" => min_incl = Some(v.parse().unwrap()),
                        "maxInclusive" => max_incl = Some(v.parse().unwrap()),
                        "minExclusive" => min_excl = Some(v.parse().unwrap()),
                        "maxExclusive" => max_excl = Some(v.parse().unwrap()),
                        "pattern" => {
                            assert!(pattern.is_none(), "multiple patterns in {name}");
                            pattern = Some(v.to_string())
                        }
                        "maxLength" => max_len = Some(v.parse().unwrap()),
                        "length" => len = Some(v.parse().unwrap()),
                        "annotation" | "simpleType" => {}
                        other => panic!("unsupported facet {other} in {name}"),
                    }
                }
                SKind::Restr { base, enums, min_incl, max_incl, min_excl, max_excl, pattern, max_len, len }
            }
            "union" => {
                let mut members: Vec<usize> = body.attribute("memberTypes").unwrap_or_default().split_whitespace().map(|m| self.type_ref(m)).collect();
                let inline: Vec<N<'a, 'i>> = xs_children(body).filter(|c| local(*c) == "simpleType").collect();
                for (k, inner) in inline.into_iter().enumerate() {
                    members.push(self.simple_type(inner, format!("{name}.member{}", k + 1)));
                }
                assert!(!members.is_empty(), "union {name} has no member types");
                SKind::Union(members)
            }
            "list" => SKind::List(self.type_ref(body.attribute("itemType").expect("list itemType"))),
            other => panic!("unsupported simpleType body {other}"),
        };
        let i = self.s.simple.len();
        self.s.simple.push(SDef { name: name.clone(), kind });
        self.s.simple_by_name.insert(name, i);
        i
    }

    fn attribute(&mut self, n: N<'a, 'i>, owner: &str) -> Attr {
        let name = n.attribute("name").expect("attribute name").to_string();
        assert!(n.attribute("ref").is_none(), "attribute refs are unsupported");
        let ty = if let Some(t) = n.attribute("type") {
            self.type_ref(t)
        } else if let Some(st) = xs_children(n).find(|c| local(*c) == "simpleType") {
            self.simple_type(st, format!("{owner}@{name}"))
        } else {
            panic!("attribute {owner}@{name} has no type")
        };
        Attr { name, ty, required: n.attribute("use") == Some("required"), default: n.attribute("default").map(str::to_string) }
    }

    fn attrs_of(&mut self, n: N<'a, 'i>, owner: &str, out: &mut Vec<Attr>) {
        for c in xs_children(n) {
            match local(c) {
                "attribute" => {
                    let a = self.attribute(c, owner);
                    out.push(a);
                }
                "attributeGroup" => {
                    let r = c.attribute("ref").expect("attributeGroup ref");
                    let g = *self.attr_groups.get(r).unwrap_or_else(|| panic!("unknown attributeGroup {r}"));
                    self.attrs_of(g, r, out);
                }
                _ => {}
            }
        }
    }

    fn particle(&self, n: N) -> Option<Particle> {
        let (min, max) = occurs(n);
        match local(n) {
            "element" => Some(Particle::Element {
                name: n.attribute("name").expect("local element name").to_string(),
                ty: n.attribute("type").expect("local element type").to_string(),
                min,
                max,
            }),
            "sequence" | "choice" => {
                let items = xs_children(n).filter_map(|c| self.particle(c)).collect();
                Some(if local(n) == "sequence" { Particle::Seq { items, min, max } } else { Particle::Choice { items, min, max } })
            }
            "group" => Some(Particle::Group { name: n.attribute("ref").expect("group ref").to_string(), min, max }),
            "annotation" | "attribute" | "attributeGroup" => None,
            other => panic!("unsupported particle {other}"),
        }
    }

    fn complex_type(&mut self, n: N<'a, 'i>, name: String) -> CDef {
        let mut attrs = Vec::new();
        let mut content = Content::Empty;
        let mut base = None;
        for c in xs_children(n) {
            match local(c) {
                "sequence" | "choice" | "group" => content = Content::Elements(self.particle(c).unwrap()),
                "complexContent" => {
                    let ext = xs_children(c).find(|e| local(*e) == "extension").expect("complexContent/extension");
                    base = Some(ext.attribute("base").unwrap().to_string());
                    for p in xs_children(ext) {
                        assert!(!matches!(local(p), "sequence" | "choice" | "group"), "complexContent extension with added particles is unsupported ({name})");
                    }
                    self.attrs_of(ext, &name, &mut attrs);
                }
                "simpleContent" => {
                    let ext = xs_children(c).find(|e| local(*e) == "extension").expect("simpleContent/extension");
                    content = Content::Simple(self.type_ref(ext.attribute("base").unwrap()));
                    self.attrs_of(ext, &name, &mut attrs);
                }
                _ => {}
            }
        }
        self.attrs_of(n, &name, &mut attrs);
        assert_ne!(n.attribute("mixed"), Some("true"), "mixed content is unsupported ({name})");
        CDef { name, attrs, content, base }
    }
}

fn parse_schema(text: &str) -> Schema {
    let doc = roxmltree::Document::parse(text).expect("XSD is not well-formed");
    let root = doc.root_element();
    let mut p = Parser {
        named_simple: HashMap::new(),
        attr_groups: HashMap::new(),
        s: Schema { simple: Vec::new(), simple_by_name: HashMap::new(), complex: Vec::new(), complex_by_name: HashMap::new(), groups: BTreeMap::new() },
    };
    for c in xs_children(root) {
        match local(c) {
            "simpleType" => {
                p.named_simple.insert(c.attribute("name").unwrap().to_string(), c);
            }
            "attributeGroup" => {
                p.attr_groups.insert(c.attribute("name").unwrap().to_string(), c);
            }
            _ => {}
        }
    }
    // named simple types in declaration order, so table indices are stable
    for c in xs_children(root).filter(|c| local(*c) == "simpleType") {
        p.type_ref(c.attribute("name").unwrap());
    }
    let mut pending_complex = Vec::new();
    for c in xs_children(root) {
        match local(c) {
            "complexType" => pending_complex.push((c.attribute("name").unwrap().to_string(), c)),
            "group" => {
                let name = c.attribute("name").unwrap().to_string();
                let body = xs_children(c).find(|x| local(*x) != "annotation").unwrap();
                let part = p.particle(body).unwrap();
                p.s.groups.insert(name, part);
            }
            "element" => {
                assert_eq!(c.attribute("name"), Some("scene"), "only the scene root element is supported");
                let ct = xs_children(c).find(|x| local(*x) == "complexType").unwrap();
                pending_complex.push(("scene".to_string(), ct));
            }
            "simpleType" | "attributeGroup" | "annotation" => {}
            other => panic!("unsupported top-level {other}"),
        }
    }
    for (name, node) in pending_complex {
        let def = p.complex_type(node, name.clone());
        p.s.complex_by_name.insert(name, p.s.complex.len());
        p.s.complex.push(def);
    }
    // complexContent extension: prepend the base's attributes and inherit its content
    for i in 0..p.s.complex.len() {
        if let Some(b) = p.s.complex[i].base.clone() {
            let bi = p.s.complex_by_name[&b];
            assert!(p.s.complex[bi].base.is_none(), "multi-level extension unsupported");
            let mut attrs = p.s.complex[bi].attrs.clone();
            attrs.append(&mut p.s.complex[i].attrs);
            p.s.complex[i].attrs = attrs;
            p.s.complex[i].content = p.s.complex[bi].content.clone();
        }
    }
    for c in &p.s.complex {
        let mut seen = HashSet::new();
        for a in &c.attrs {
            assert!(seen.insert(a.name.clone()), "duplicate attribute {}@{}", c.name, a.name);
        }
    }
    p.s
}

fn root_builtin(s: &Schema, mut i: usize) -> &'static str {
    loop {
        match &s.simple[i].kind {
            SKind::Builtin(b) => return b,
            SKind::Restr { base, .. } => i = *base,
            SKind::Union(_) => return "Union",
            SKind::List(_) => return "List",
        }
    }
}

fn expand_particle(s: &Schema, p: &Particle) -> Particle {
    match p {
        Particle::Element { .. } => p.clone(),
        Particle::Seq { items, min, max } => Particle::Seq { items: items.iter().map(|x| expand_particle(s, x)).collect(), min: *min, max: *max },
        Particle::Choice { items, min, max } => Particle::Choice { items: items.iter().map(|x| expand_particle(s, x)).collect(), min: *min, max: *max },
        Particle::Group { name, min, max } => {
            let inner = expand_particle(s, &s.groups[name]);
            if (*min, *max) == (1, 1) {
                return inner;
            }
            match inner {
                Particle::Choice { items, min: 1, max: 1 } => Particle::Choice { items, min: *min, max: *max },
                Particle::Seq { items, min: 1, max: 1 } => Particle::Seq { items, min: *min, max: *max },
                other => Particle::Seq { items: vec![other], min: *min, max: *max },
            }
        }
    }
}

fn opt<T: std::fmt::Debug>(v: &Option<T>) -> String {
    match v {
        Some(x) => format!("Some({x:?})"),
        None => "None".into(),
    }
}

fn emit_particle(s: &Schema, p: &Particle, out: &mut String) {
    let m = |x: u32| if x == UNBOUNDED { "UNBOUNDED".to_string() } else { x.to_string() };
    match p {
        Particle::Element { name, ty, min, max } => {
            let ti = s.complex_by_name.get(ty).unwrap_or_else(|| panic!("element {name} has unknown type {ty}"));
            write!(out, "Particle::Element {{ name: {name:?}, ty: {ti}, min: {}, max: {} }}", m(*min), m(*max)).unwrap();
        }
        Particle::Seq { items, min, max } | Particle::Choice { items, min, max } => {
            let kind = if matches!(p, Particle::Seq { .. }) { "Seq" } else { "Choice" };
            write!(out, "Particle::{kind} {{ items: &[").unwrap();
            for it in items {
                emit_particle(s, it, out);
                out.push_str(", ");
            }
            write!(out, "], min: {}, max: {} }}", m(*min), m(*max)).unwrap();
        }
        Particle::Group { .. } => panic!("groups are expanded"),
    }
}

fn id_kind(s: &Schema, i: usize) -> &'static str {
    match root_builtin(s, i) {
        "Id" => "IdKind::Id",
        "IdRef" => "IdKind::IdRef",
        "IdRefs" => "IdKind::IdRefs",
        _ => "IdKind::None",
    }
}

fn emit_tables(s: &Schema) -> String {
    let mut o = String::new();
    o.push_str(
        "// @generated from schema/sr-core/scene-render.xsd by the `tables_match_schema` test\n\
         // (`FILMCRAFT_BLESS=1 cargo test -p filmcraft-interchange tables_match_schema`). Do not edit.\n\n",
    );
    o.push_str("/// Every simple type of the schema; builtins first as they are referenced.\n");
    o.push_str("pub static SIMPLE_TYPES: &[SimpleType] = &[\n");
    for d in &s.simple {
        let kind = match &d.kind {
            SKind::Builtin(b) => format!("SimpleKind::Builtin(Builtin::{b})"),
            SKind::Restr { base, enums, min_incl, max_incl, min_excl, max_excl, pattern, max_len, len } => format!(
                "SimpleKind::Restriction(Restriction {{ base: {base}, enums: &{enums:?}, min_inclusive: {}, \
                 max_inclusive: {}, min_exclusive: {}, max_exclusive: {}, pattern: {}, max_length: {}, length: {} }})",
                opt(min_incl),
                opt(max_incl),
                opt(min_excl),
                opt(max_excl),
                opt(pattern),
                opt(max_len),
                opt(len),
            ),
            SKind::Union(m) => format!("SimpleKind::Union(&{m:?})"),
            SKind::List(it) => format!("SimpleKind::List({it})"),
        };
        writeln!(o, "    SimpleType {{ name: {:?}, kind: {kind} }},", d.name).unwrap();
    }
    o.push_str("];\n\n");

    o.push_str("/// Every complex type of the schema, extensions applied and groups expanded.\n");
    o.push_str("pub static COMPLEX_TYPES: &[ComplexType] = &[\n");
    for c in &s.complex {
        write!(o, "    ComplexType {{ name: {:?}, attrs: &[", c.name).unwrap();
        for a in &c.attrs {
            write!(o, "AttrDecl {{ name: {:?}, ty: {}, required: {}, default: {}, id: {} }}, ", a.name, a.ty, a.required, opt(&a.default), id_kind(s, a.ty))
                .unwrap();
        }
        o.push_str("], content: ");
        match &c.content {
            Content::Empty => o.push_str("Content::Empty"),
            Content::Simple(t) => write!(o, "Content::Simple({t})").unwrap(),
            Content::Elements(p) => {
                o.push_str("Content::Elements(&");
                emit_particle(s, &expand_particle(s, p), &mut o);
                o.push(')');
            }
        }
        o.push_str(" },\n");
    }
    o.push_str("];\n\n");
    o.push_str("/// Index of the anonymous complex type of the `scene` root element.\n");
    writeln!(o, "pub const ROOT_TYPE: usize = {};", s.complex_by_name["scene"]).unwrap();
    o
}

#[test]
fn tables_match_schema() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let xsd = std::fs::read_to_string(dir.join("schema/sr-core/scene-render.xsd")).unwrap();
    let want = emit_tables(&parse_schema(&xsd));
    let path = dir.join("src/scene/validate/tables.rs");
    let bless = std::env::var("FILMCRAFT_BLESS").is_ok_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"));
    if bless {
        std::fs::write(&path, &want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(have == want, "{} is stale: rerun with FILMCRAFT_BLESS=1", path.display());
}
