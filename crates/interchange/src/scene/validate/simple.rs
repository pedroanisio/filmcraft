//! Validation of lexical values against XSD simple types.
//!
//! Adapted from scene-render `crates/sr-model/src/xsd/simple.rs` (revision c0c668b8,
//! © Pedro Anisio Silva), relicensed by its author for FilmCraft under MIT OR Apache-2.0; see
//! `ATTRIBUTION.md`.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::regex::Regex;
use super::{Builtin, MAX_TYPE_DEPTH, Restriction, SIMPLE_TYPES, SimpleKind, parse_xsd_double, parse_xsd_integer, root_builtin, simple_type};

/// Collapses XML whitespace: trims and folds runs of `#x20 #x9 #xA #xD` to one space.
pub fn collapse(s: &str) -> String {
    s.split([' ', '\t', '\n', '\r']).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ")
}

fn preserves_whitespace(ty: usize) -> bool {
    matches!(root_builtin(ty), Some(Builtin::String))
}

/// Compiled `xs:pattern` of every restriction that has one; a pattern that fails to compile is
/// absent, which the table test rules out.
fn patterns() -> &'static HashMap<usize, Regex> {
    static P: OnceLock<HashMap<usize, Regex>> = OnceLock::new();
    P.get_or_init(|| {
        SIMPLE_TYPES
            .iter()
            .enumerate()
            .filter_map(|(i, t)| match t.kind {
                SimpleKind::Restriction(Restriction { pattern: Some(p), .. }) => Regex::new(p).ok().map(|re| (i, re)),
                _ => None,
            })
            .collect()
    })
}

/// Human-readable name of a simple type for messages.
pub fn describe(ty: usize) -> String {
    let Some(t) = simple_type(ty) else { return "value".to_string() };
    match t.kind {
        SimpleKind::Restriction(r) if t.name.contains('@') => simple_type(r.base).map_or(t.name, |b| b.name).to_string(),
        _ => t.name.to_string(),
    }
}

/// Checks `raw` (an attribute value or text content) against simple type `ty`, returning a message
/// describing the first violation.
pub fn check(ty: usize, raw: &str) -> Result<(), String> {
    check_depth(ty, raw, 0)
}

fn check_depth(ty: usize, raw: &str, depth: usize) -> Result<(), String> {
    let already_collapsed = !raw.starts_with(' ') && !raw.ends_with(' ') && !raw.contains("  ") && !raw.contains(['\t', '\n', '\r']);
    if preserves_whitespace(ty) || already_collapsed { check_normalized(ty, raw, depth) } else { check_normalized(ty, &collapse(raw), depth) }
}

fn number(v: &str) -> Option<f64> {
    parse_xsd_integer(v).map(|i| i as f64).or_else(|| parse_xsd_double(v))
}

// Bounds are written as `!(x >= m)` so that NaN fails every facet.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn check_normalized(ty: usize, v: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_TYPE_DEPTH {
        return Err("type definitions nest too deeply".into());
    }
    let Some(t) = simple_type(ty) else { return Err("unknown type".into()) };
    match t.kind {
        SimpleKind::Builtin(b) => check_builtin(b, v),
        SimpleKind::Restriction(r) => {
            check_normalized(r.base, v, depth + 1)?;
            let numeric = matches!(
                root_builtin(r.base),
                Some(Builtin::Double | Builtin::Integer | Builtin::Int | Builtin::NonNegativeInteger | Builtin::PositiveInteger | Builtin::UnsignedLong)
            );
            if !r.enums.is_empty() {
                let ok = if numeric {
                    let x = number(v);
                    r.enums.iter().any(|e| {
                        let ev = number(e);
                        ev.is_some() && ev == x
                    })
                } else {
                    r.enums.contains(&v)
                };
                if !ok {
                    return Err(format!("{v:?} is not one of: {}", r.enums.join(", ")));
                }
            }
            if numeric {
                let x = number(v).unwrap_or(f64::NAN);
                if let Some(m) = r.min_inclusive
                    && !(x >= m)
                {
                    return Err(format!("{v} is below the minimum {m}"));
                }
                if let Some(m) = r.max_inclusive
                    && !(x <= m)
                {
                    return Err(format!("{v} is above the maximum {m}"));
                }
                if let Some(m) = r.min_exclusive
                    && !(x > m)
                {
                    return Err(format!("{v} must be greater than {m}"));
                }
                if let Some(m) = r.max_exclusive
                    && !(x < m)
                {
                    return Err(format!("{v} must be less than {m}"));
                }
            }
            if let Some(max) = r.max_length {
                let n = v.chars().count();
                if n > max {
                    return Err(format!("value has {n} characters; the maximum is {max}"));
                }
            }
            if let Some(len) = r.length {
                let is_list = simple_type(r.base).is_some_and(|b| matches!(b.kind, SimpleKind::List(_)));
                let n = if is_list { v.split(' ').filter(|t| !t.is_empty()).count() } else { v.chars().count() };
                if n != len {
                    return Err(format!("{v:?} has length {n}; the length must be {len}"));
                }
            }
            if r.pattern.is_some() && !patterns().get(&ty).is_some_and(|re| re.is_match(v)) {
                return Err(format!("{v:?} is not a valid {}", describe(ty)));
            }
            Ok(())
        }
        SimpleKind::Union(members) => {
            if members.iter().any(|&m| check_depth(m, v, depth + 1).is_ok()) {
                Ok(())
            } else {
                Err(format!("{v:?} is not a valid {}", describe(ty)))
            }
        }
        SimpleKind::List(item) => {
            for tok in v.split(' ').filter(|t| !t.is_empty()) {
                check_depth(item, tok, depth + 1).map_err(|e| format!("list item {tok:?}: {e}"))?;
            }
            Ok(())
        }
    }
}

/// String types whose patterns hold numbers: lengths with a unit and coordinate pairs.
const NUMERIC_PATTERN_TYPES: &[&str] = &["relativeLength", "positiveRelativeLength", "pointType", "geoPointsType"];

/// The first number of a valid `raw` value of type `ty` that is not finite: `INF`, `-INF`, `NaN`,
/// or a literal too large for a double. Looks into union members, list items and the string types
/// that hold numbers.
pub fn non_finite(ty: usize, raw: &str) -> Option<String> {
    let v = collapse(raw);
    non_finite_normalized(ty, &v, 0).map(|t| t.chars().take(40).collect())
}

fn non_finite_normalized(ty: usize, v: &str, depth: usize) -> Option<&str> {
    if depth > MAX_TYPE_DEPTH {
        return None;
    }
    let t = simple_type(ty)?;
    match t.kind {
        SimpleKind::Builtin(Builtin::Double) => parse_xsd_double(v).is_some_and(|x| !x.is_finite()).then_some(v),
        SimpleKind::Builtin(_) => None,
        SimpleKind::Restriction(_) if NUMERIC_PATTERN_TYPES.contains(&t.name) => {
            v.split(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '+' | '-' | 'e' | 'E'))).find(|n| n.parse::<f64>().is_ok_and(|x| !x.is_finite()))
        }
        SimpleKind::Restriction(r) => non_finite_normalized(r.base, v, depth + 1),
        SimpleKind::Union(members) => {
            members.iter().find(|&&m| check_normalized(m, v, depth + 1).is_ok()).and_then(|&m| non_finite_normalized(m, v, depth + 1))
        }
        SimpleKind::List(item) => v.split(' ').find_map(|tok| non_finite_normalized(item, tok, depth + 1)),
    }
}

fn check_builtin(b: Builtin, v: &str) -> Result<(), String> {
    let integer = || parse_xsd_integer(v).ok_or_else(|| format!("{v:?} is not an integer"));
    match b {
        Builtin::String => Ok(()),
        // a % must start a two-digit hexadecimal escape (libxml2 parses the URI)
        Builtin::AnyUri => {
            let b = v.as_bytes();
            let bad = b
                .iter()
                .enumerate()
                .any(|(i, &c)| c == b'%' && !(b.get(i + 1).is_some_and(u8::is_ascii_hexdigit) && b.get(i + 2).is_some_and(u8::is_ascii_hexdigit)));
            if bad { Err(format!("{v:?} is not a valid URI (a % must start a %XX escape)")) } else { Ok(()) }
        }
        Builtin::Double => parse_xsd_double(v).map(|_| ()).ok_or_else(|| format!("{v:?} is not a number")),
        // [+-]? (digits (. digits?)? | . digits), of any length: no exponent, INF or NaN
        Builtin::Decimal => {
            let body = v.strip_prefix(['+', '-']).unwrap_or(v);
            let (int, frac) = body.split_once('.').unwrap_or((body, ""));
            let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
            if (int.is_empty() && frac.is_empty()) || !digits(int) || !digits(frac) { Err(format!("{v:?} is not a decimal")) } else { Ok(()) }
        }
        Builtin::Boolean => match v {
            "true" | "false" | "1" | "0" => Ok(()),
            _ => Err(format!("{v:?} is not a boolean (true, false, 1 or 0)")),
        },
        Builtin::Integer => {
            let i = integer()?;
            if i < i128::from(i64::MIN) || i > i128::from(i64::MAX) {
                return Err(format!("{v} is outside the supported integer range"));
            }
            Ok(())
        }
        Builtin::Int => {
            let i = integer()?;
            if i < i128::from(i32::MIN) || i > i128::from(i32::MAX) {
                return Err(format!("{v} is outside the xs:int range"));
            }
            Ok(())
        }
        Builtin::NonNegativeInteger | Builtin::UnsignedLong => {
            let i = integer()?;
            if i < 0 {
                return Err(format!("{v} is negative"));
            }
            if i > i128::from(u64::MAX) {
                return Err(format!("{v} is outside the supported integer range"));
            }
            Ok(())
        }
        Builtin::PositiveInteger => {
            let i = integer()?;
            if i < 1 {
                return Err(format!("{v} is not a positive integer"));
            }
            if i > i128::from(u64::MAX) {
                return Err(format!("{v} is outside the supported integer range"));
            }
            Ok(())
        }
        Builtin::Id | Builtin::IdRef | Builtin::NcName => {
            if is_ncname(v) {
                Ok(())
            } else {
                Err(format!("{v:?} is not a valid XML name (NCName)"))
            }
        }
        Builtin::IdRefs => {
            if v.is_empty() {
                return Err("expected at least one ID reference".into());
            }
            match v.split(' ').find(|t| !is_ncname(t)) {
                Some(t) => Err(format!("{t:?} is not a valid XML name (NCName)")),
                None => Ok(()),
            }
        }
        Builtin::NmToken => {
            if is_nmtoken(v) {
                Ok(())
            } else {
                Err(format!("{v:?} is not a valid name token (NMTOKEN)"))
            }
        }
        Builtin::NmTokens => {
            if v.is_empty() {
                return Err("expected at least one name token".into());
            }
            match v.split(' ').find(|t| !is_nmtoken(t)) {
                Some(t) => Err(format!("{t:?} is not a valid name token (NMTOKEN)")),
                None => Ok(()),
            }
        }
        Builtin::DateTime => {
            if is_datetime(v) {
                Ok(())
            } else {
                Err(format!("{v:?} is not an xs:dateTime (YYYY-MM-DDThh:mm:ss[.fff][Z|±hh:mm])"))
            }
        }
    }
}

fn is_name_start(c: char) -> bool {
    matches!(c,
        'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// `NCName`: an XML name without colons.
pub fn is_ncname(s: &str) -> bool {
    let mut it = s.chars();
    matches!(it.next(), Some(c) if is_name_start(c)) && it.all(is_name_char)
}

/// `NMTOKEN`: one or more name characters.
pub fn is_nmtoken(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| is_name_char(c) || c == ':')
}

fn two(b: &[u8], i: usize) -> Option<u32> {
    match (b.get(i), b.get(i + 1)) {
        (Some(&x), Some(&y)) if x.is_ascii_digit() && y.is_ascii_digit() => Some(u32::from(x - b'0') * 10 + u32::from(y - b'0')),
        _ => None,
    }
}

/// `xs:dateTime`: `-?YYYY-MM-DDThh:mm:ss(.s+)?(Z|[+-]hh:mm)?`, fields in range.
fn is_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    let at = |i: usize, c: u8| b.get(i) == Some(&c);
    let mut i = usize::from(at(0, b'-'));
    let ys = i;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    let ylen = i - ys;
    if ylen < 4 || (ylen > 4 && at(ys, b'0')) {
        return false;
    }
    if !at(i, b'-') || !at(i + 3, b'-') || !at(i + 6, b'T') || !at(i + 9, b':') || !at(i + 12, b':') {
        return false;
    }
    let (Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (two(b, i + 1), two(b, i + 4), two(b, i + 7), two(b, i + 10), two(b, i + 13)) else {
        return false;
    };
    i += 15;
    let mut frac_zero = true;
    if at(i, b'.') {
        i += 1;
        let fs = i;
        while let Some(c) = b.get(i).filter(|c| c.is_ascii_digit()) {
            if *c != b'0' {
                frac_zero = false;
            }
            i += 1;
        }
        if i == fs {
            return false;
        }
    }
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || mi > 59 || se > 59 {
        return false;
    }
    if h > 24 || (h == 24 && (mi != 0 || se != 0 || !frac_zero)) {
        return false;
    }
    match b.get(i..) {
        Some([]) | Some([b'Z']) => true,
        Some([s, _, _, b':', _, _]) if *s == b'+' || *s == b'-' => two(b, i + 1).is_some_and(|hh| hh <= 14) && two(b, i + 4).is_some_and(|mm| mm <= 59),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn ty(name: &str) -> usize {
        SIMPLE_TYPES.iter().position(|t| t.name == name).unwrap_or_else(|| panic!("no type {name}"))
    }

    #[test]
    fn doubles() {
        let d = ty("xs:double");
        for ok in ["1", "-1.5", ".5", "5.", "1e3", "+2E-2", "INF", "-INF", "NaN", " 3 "] {
            assert!(check(d, ok).is_ok(), "{ok}");
        }
        for bad in ["", "e3", "1e", "inf", "+INF", "1,5", "0x10", "."] {
            assert!(check(d, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn bounded() {
        let u = ty("unitDecimal");
        assert!(check(u, "0").is_ok());
        assert!(check(u, "1").is_ok());
        assert!(check(u, "1.0001").is_err());
        assert!(check(u, "NaN").is_err());
        let p = ty("positiveDecimal");
        assert!(check(p, "0").is_err());
        assert!(check(p, "1e-9").is_ok());
    }

    #[test]
    fn colours_and_lengths() {
        let c = ty("colorType");
        for ok in ["#ff0000", "#FF0000CC", "1,0.5,0", "1,0.5,0,1", ".5,.5,.5", "var(--brand-1)"] {
            assert!(check(c, ok).is_ok(), "{ok}");
        }
        for bad in ["#ff00", "1.5,0,0", "1,0", "red", "var(brand)"] {
            assert!(check(c, bad).is_err(), "{bad}");
        }
        let l = ty("lengthType");
        for ok in ["10", "-3.5", "50%", "10vw", "2.5vmin", "1e2"] {
            assert!(check(l, ok).is_ok(), "{ok}");
        }
        assert!(check(l, "10px").is_err());
        let pl = ty("positiveLengthType");
        assert!(check(pl, "0%").is_err());
        assert!(check(pl, "0").is_err());
        assert!(check(pl, "0.1%").is_ok());
    }

    #[test]
    fn names_and_dates() {
        assert!(is_ncname("layer_1"));
        assert!(is_ncname("é-x"));
        assert!(!is_ncname("1layer"));
        assert!(!is_ncname("a:b"));
        assert!(is_datetime("2026-09-27T10:00:00Z"));
        assert!(is_datetime("2026-09-27T10:00:00.25+02:00"));
        assert!(!is_datetime("2026-13-01T00:00:00"));
        assert!(!is_datetime("2026-09-27"));
        assert!(!is_datetime("2026-09-27T10:00:00+2:00"));
        assert!(!is_datetime("-"));
    }

    #[test]
    fn non_finite_numbers() {
        assert_eq!(non_finite(ty("xs:double"), "INF").as_deref(), Some("INF"));
        assert_eq!(non_finite(ty("xs:double"), "1"), None);
        assert_eq!(non_finite(ty("pointType"), "1e999,0").as_deref(), Some("1e999"));
    }
}
