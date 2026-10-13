//! Lexical parsers for the sr-core scene simple types (`scene-render.xsd` 1.7.0).
//!
//! Every parser takes an attribute value as the document spells it and returns either the typed
//! value or a message naming what is wrong. They never panic and never produce a non-finite number:
//! Rust's `f64::from_str` accepts `inf`, `nan` and `infinity`, which XSD spells `INF`/`NaN` and which
//! no scene quantity allows, so [`double`] checks the XSD lexical space first.
//!
//! The hand-written matchers mirror the schema's `xs:pattern` facets exactly; the equivalence tests
//! live next to the validator (S3).

use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};

/// A value that does not match its type.
pub type ValueResult<T> = std::result::Result<T, String>;

/// Largest frame-rate numerator or denominator accepted (well above any real rate; keeps the
/// rational arithmetic in `FrameRate` far from overflow).
pub const MAX_FPS_TERM: i64 = 1_000_000;

/// Longest number list (`numberListType`, `pointListType`) parsed in one attribute.
pub const MAX_LIST_ITEMS: usize = 65_536;

/// XML Schema `whiteSpace="collapse"` for the atomic types: leading and trailing XML whitespace
/// is ignored (internal runs only matter for lists, which split on them).
fn collapse(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `xs:double` lexical form without the special values:
/// `(+|-)?([0-9]+(\.[0-9]*)?|\.[0-9]+)([Ee](+|-)?[0-9]+)?`.
fn double_lexical(s: &str) -> bool {
    let s = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (mantissa, exp) = match s.find(['e', 'E']) {
        Some(i) => (s.get(..i).unwrap_or(""), Some(s.get(i + 1..).unwrap_or(""))),
        None => (s, None),
    };
    let mantissa_ok = match mantissa.split_once('.') {
        Some((int, frac)) => (digits(int) && (frac.is_empty() || digits(frac))) || (int.is_empty() && digits(frac)),
        None => digits(mantissa),
    };
    let exp_ok = exp.is_none_or(|e| digits(e.strip_prefix(['+', '-']).unwrap_or(e)));
    mantissa_ok && exp_ok
}

/// A finite `xs:double`. `INF`, `-INF` and `NaN` are lexically valid XSD but rejected here: no
/// scene attribute can use them.
pub fn double(s: &str) -> ValueResult<f64> {
    let t = collapse(s);
    if matches!(t, "INF" | "+INF" | "-INF" | "NaN") {
        return Err(format!("{t:?} is not a finite number"));
    }
    if !double_lexical(t) {
        return Err(format!("{s:?} is not a number"));
    }
    match t.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        // e.g. "1e999": lexically fine but outside f64
        _ => Err(format!("{s:?} is out of range")),
    }
}

/// A finite double inside `[lo, hi]` (inclusive bounds; pass `f64::NEG_INFINITY`/`INFINITY` for open ends).
pub fn double_in(s: &str, lo: f64, hi: f64) -> ValueResult<f64> {
    let v = double(s)?;
    if v < lo || v > hi {
        return Err(format!("{v} is outside [{lo}, {hi}]"));
    }
    Ok(v)
}

/// `positiveDecimal`: a double > 0.
pub fn positive(s: &str) -> ValueResult<f64> {
    let v = double(s)?;
    if v <= 0.0 {
        return Err(format!("{v} must be greater than 0"));
    }
    Ok(v)
}

/// `nonNegativeDecimal`: a double ≥ 0.
pub fn non_negative(s: &str) -> ValueResult<f64> {
    double_in(s, 0.0, f64::INFINITY)
}

/// `unitDecimal`: a double in [0, 1].
pub fn unit(s: &str) -> ValueResult<f64> {
    double_in(s, 0.0, 1.0)
}

/// `durationType`: seconds in (0, 1 000 000].
pub fn duration(s: &str) -> ValueResult<f64> {
    let v = positive(s)?;
    if v > 1_000_000.0 {
        return Err(format!("duration {v} exceeds 1000000 s"));
    }
    Ok(v)
}

/// `decibelType`: a gain in [-120, 24] dB.
pub fn decibel(s: &str) -> ValueResult<f64> {
    double_in(s, -120.0, 24.0)
}

/// `xs:boolean`: `true`, `false`, `1`, `0`.
pub fn boolean(s: &str) -> ValueResult<bool> {
    match collapse(s) {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err(format!("{s:?} is not a boolean")),
    }
}

/// An `xs:integer`-derived value within `[lo, hi]` (`xs:nonNegativeInteger`, `xs:positiveInteger`,
/// `xs:unsignedLong`, … restricted to what fits an `i64`).
pub fn integer_in(s: &str, lo: i64, hi: i64) -> ValueResult<i64> {
    let t = collapse(s);
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    if !digits(body) {
        return Err(format!("{s:?} is not an integer"));
    }
    let v = t.parse::<i64>().map_err(|_| format!("{s:?} is out of range"))?;
    if v < lo || v > hi {
        return Err(format!("{v} is outside [{lo}, {hi}]"));
    }
    Ok(v)
}

/// `xs:unsignedLong` (seeds).
pub fn unsigned_long(s: &str) -> ValueResult<u64> {
    let t = collapse(s);
    let body = t.strip_prefix('+').unwrap_or(t);
    if !digits(body) {
        return Err(format!("{s:?} is not a non-negative integer"));
    }
    body.parse::<u64>().map_err(|_| format!("{s:?} is out of range"))
}

/// Seconds → exact [`Tick`], refusing values that would not fit (`Tick::from_seconds_f64` does not check).
/// Rounds half to even, so a value exactly between two ticks has one deterministic answer.
pub fn seconds_to_tick(s: f64) -> Option<Tick> {
    let t = s * TICKS_PER_SECOND as f64;
    if !t.is_finite() || t.abs() > Tick::MAX.0 as f64 {
        return None;
    }
    Some(Tick(t.round_ties_even() as i64))
}

/// An `xs:double` seconds attribute as a [`Tick`].
pub fn seconds(s: &str) -> ValueResult<Tick> {
    let v = double(s)?;
    seconds_to_tick(v).ok_or_else(|| format!("{v} s is outside the representable time range"))
}

/// `fpsType`: `[1-9][0-9]*(/[1-9][0-9]*)?`, e.g. `30`, `30000/1001`. Terms above [`MAX_FPS_TERM`]
/// are refused.
pub fn fps(s: &str) -> ValueResult<FrameRate> {
    // pattern facets apply to the whitespace-collapsed value
    let t = collapse(s);
    let (num, den) = t.split_once('/').unwrap_or((t, "1"));
    let term = |x: &str| -> ValueResult<i64> {
        if !digits(x) || x.starts_with('0') {
            return Err(format!("{s:?} is not a frame rate (N or N/D)"));
        }
        match x.parse::<i64>() {
            Ok(v) if v <= MAX_FPS_TERM => Ok(v),
            _ => Err(format!("frame rate term {x} exceeds {MAX_FPS_TERM}")),
        }
    };
    Ok(FrameRate::new(term(num)?, term(den)?))
}

/// The unit of a [`Length`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LengthUnit {
    /// Plain number: pixels.
    Px,
    /// Percent of the parent box.
    Percent,
    /// Percent of the output frame width.
    Vw,
    /// Percent of the output frame height.
    Vh,
    /// Percent of the smaller output dimension.
    Vmin,
    /// Percent of the larger output dimension.
    Vmax,
}

/// `lengthType` / `positiveLengthType`: a number of pixels or a relative length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Length {
    pub value: f64,
    pub unit: LengthUnit,
}

impl Length {
    pub const fn px(value: f64) -> Length {
        Length { value, unit: LengthUnit::Px }
    }

    /// Pixels, given the parent box extent along this axis and the output frame size.
    /// Every input is finite, so the result is finite for any frame up to `f64` range.
    pub fn resolve(self, parent: f64, frame_w: f64, frame_h: f64) -> f64 {
        let pct = self.value / 100.0;
        match self.unit {
            LengthUnit::Px => self.value,
            LengthUnit::Percent => pct * parent,
            LengthUnit::Vw => pct * frame_w,
            LengthUnit::Vh => pct * frame_h,
            LengthUnit::Vmin => pct * frame_w.min(frame_h),
            LengthUnit::Vmax => pct * frame_w.max(frame_h),
        }
    }
}

const UNITS: [(&str, LengthUnit); 5] = [
    // longest first so "vmin" is not read as "vm" + "in"
    ("vmin", LengthUnit::Vmin),
    ("vmax", LengthUnit::Vmax),
    ("vw", LengthUnit::Vw),
    ("vh", LengthUnit::Vh),
    ("%", LengthUnit::Percent),
];

/// `relativeLength` number part: `-?([0-9]+(\.[0-9]*)?|\.[0-9]+)` (no exponent, no `+`).
fn relative_number(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    match s.split_once('.') {
        Some((int, frac)) => (digits(int) && (frac.is_empty() || digits(frac))) || (int.is_empty() && digits(frac)),
        None => digits(s),
    }
}

fn split_unit(t: &str) -> Option<(&str, LengthUnit)> {
    UNITS.iter().find_map(|(suffix, unit)| t.strip_suffix(*suffix).map(|n| (n, *unit)))
}

/// `lengthType` = `xs:double | relativeLength`.
pub fn length(s: &str) -> ValueResult<Length> {
    let t = collapse(s);
    if let Some((num, unit)) = split_unit(t) {
        if !relative_number(num) {
            return Err(format!("{s:?} is not a length"));
        }
        let value = num.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| format!("{s:?} is out of range"))?;
        return Ok(Length { value, unit });
    }
    double(t).map(Length::px).map_err(|_| format!("{s:?} is not a length (number, %, vw, vh, vmin or vmax)"))
}

/// `positiveLengthType` = `positiveDecimal | positiveRelativeLength`.
pub fn positive_length(s: &str) -> ValueResult<Length> {
    let l = length(s)?;
    if l.value <= 0.0 {
        return Err(format!("{s:?} must be greater than 0"));
    }
    Ok(l)
}

/// Straight-alpha RGBA, each channel in [0, 1].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub [f32; 4]);

impl Rgba {
    pub const BLACK: Rgba = Rgba([0.0, 0.0, 0.0, 1.0]);
}

/// `paintType` (and `colorType`, which is its first member).
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Color(Rgba),
    /// `var(--name)`: a style token, resolved against `styles/token` by the semantic pass.
    Var(String),
    /// `url(#id)`: a gradient, pattern or noise in `paints`.
    Ref(String),
}

fn hex_byte(s: &str) -> Option<f32> {
    u8::from_str_radix(s, 16).ok().map(|b| f32::from(b) / 255.0)
}

/// One normalised channel: `0(\.[0-9]+)?|1(\.0+)?|\.[0-9]+`.
fn unit_channel(s: &str) -> Option<f32> {
    let ok = match s.split_once('.') {
        Some(("0", frac)) => digits(frac),
        Some(("1", frac)) => !frac.is_empty() && frac.bytes().all(|b| b == b'0'),
        Some(("", frac)) => digits(frac),
        Some(_) => false,
        None => s == "0" || s == "1",
    };
    if !ok {
        return None;
    }
    s.parse::<f32>().ok().filter(|v| (0.0..=1.0).contains(v))
}

fn token_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `colorType`: `#RRGGBB`, `#RRGGBBAA`, 3–4 normalised channels, or `var(--name)`.
pub fn color(s: &str) -> ValueResult<Paint> {
    let t = collapse(s);
    let bad = || format!("{s:?} is not a colour (#RRGGBB[AA], r,g,b[,a] in 0..1, or var(--name))");
    if let Some(hex) = t.strip_prefix('#') {
        if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let ch = |i: usize| hex.get(i..i + 2).and_then(hex_byte);
        let (r, g, b) = (ch(0).ok_or_else(bad)?, ch(2).ok_or_else(bad)?, ch(4).ok_or_else(bad)?);
        let a = if hex.len() == 8 { ch(6).ok_or_else(bad)? } else { 1.0 };
        return Ok(Paint::Color(Rgba([r, g, b, a])));
    }
    if let Some(name) = t.strip_prefix("var(--").and_then(|r| r.strip_suffix(')')) {
        return if token_name(name) { Ok(Paint::Var(name.to_owned())) } else { Err(bad()) };
    }
    let parts: Vec<&str> = t.splitn(5, ',').collect();
    if !(3..=4).contains(&parts.len()) {
        return Err(bad());
    }
    let mut c = [0.0, 0.0, 0.0, 1.0];
    for (dst, p) in c.iter_mut().zip(&parts) {
        *dst = unit_channel(p).ok_or_else(bad)?;
    }
    Ok(Paint::Color(Rgba(c)))
}

/// `paintRefType` id: `[A-Za-z_][A-Za-z0-9_.\-]*`.
fn paint_ref_id(s: &str) -> bool {
    let mut b = s.bytes();
    b.next().is_some_and(|c| c.is_ascii_alphabetic() || c == b'_') && b.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// `paintType` = `colorType | url(#id)`.
pub fn paint(s: &str) -> ValueResult<Paint> {
    let t = collapse(s);
    if let Some(id) = t.strip_prefix("url(#").and_then(|r| r.strip_suffix(')')) {
        return if paint_ref_id(id) { Ok(Paint::Ref(id.to_owned())) } else { Err(format!("{s:?} is not a paint reference url(#id)")) };
    }
    color(t)
}

/// `pointType` number: `-?[0-9]*\.?[0-9]+([eE][-+]?[0-9]+)?`.
fn point_number(s: &str) -> Option<f64> {
    let body = s.strip_prefix('-').unwrap_or(s);
    let (mantissa, exp) = match body.find(['e', 'E']) {
        Some(i) => (body.get(..i)?, Some(body.get(i + 1..)?)),
        None => (body, None),
    };
    let mantissa_ok = match mantissa.split_once('.') {
        Some((int, frac)) => (int.is_empty() || digits(int)) && digits(frac),
        None => digits(mantissa),
    };
    let exp_ok = exp.is_none_or(|e| digits(e.strip_prefix(['+', '-']).unwrap_or(e)));
    if !(mantissa_ok && exp_ok) {
        return None;
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `pointType`: `"x,y"`.
pub fn point(s: &str) -> ValueResult<(f64, f64)> {
    let t = collapse(s);
    let bad = || format!("{s:?} is not a point \"x,y\"");
    let (x, y) = t.split_once(',').ok_or_else(bad)?;
    Ok((point_number(x).ok_or_else(bad)?, point_number(y).ok_or_else(bad)?))
}

fn list_items(s: &str) -> ValueResult<Vec<&str>> {
    let items: Vec<&str> = s.split_ascii_whitespace().take(MAX_LIST_ITEMS.saturating_add(1)).collect();
    if items.len() > MAX_LIST_ITEMS {
        return Err(format!("list has more than {MAX_LIST_ITEMS} items"));
    }
    Ok(items)
}

/// `numberListType`: whitespace-separated finite doubles.
pub fn number_list(s: &str) -> ValueResult<Vec<f64>> {
    list_items(s)?.into_iter().map(double).collect()
}

/// `pointListType`: whitespace-separated `x,y` points.
pub fn point_list(s: &str) -> ValueResult<Vec<(f64, f64)>> {
    list_items(s)?.into_iter().map(point).collect()
}

/// `timecodeType`: `HH:MM:SS:FF` or `HH:MM:SS;FF` (drop frame). Returns (h, m, s, f, drop).
pub fn timecode(s: &str) -> ValueResult<(u32, u32, u32, u32, bool)> {
    let t = collapse(s).as_bytes();
    let bad = || format!("{s:?} is not a timecode HH:MM:SS:FF");
    let [h1, h2, b':', m1, m2, b':', s1, s2, sep, f1, f2] = *t else { return Err(bad()) };
    let d = |a: u8, b: u8| -> Option<u32> { (a.is_ascii_digit() && b.is_ascii_digit()).then(|| u32::from(a - b'0') * 10 + u32::from(b - b'0')) };
    let (h, m, sec, f) = (d(h1, h2).ok_or_else(bad)?, d(m1, m2).ok_or_else(bad)?, d(s1, s2).ok_or_else(bad)?, d(f1, f2).ok_or_else(bad)?);
    if m > 59 || sec > 59 {
        return Err(bad());
    }
    let drop = match sep {
        b':' => false,
        b';' => true,
        _ => return Err(bad()),
    };
    Ok((h, m, sec, f, drop))
}

/// `sha256Type`: 64 lower-case hex digits.
pub fn sha256_hex(s: &str) -> ValueResult<&str> {
    let t = collapse(s);
    if t.len() == 64 && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(t)
    } else {
        Err(format!("{s:?} is not a lower-case SHA-256 hex digest"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_follow_xsd_not_rust() {
        for ok in ["0", "-1.5", "+2", "1.", ".5", "1e3", "1E-3", "-.5e+2", " 7 ", "1e308"] {
            assert!(double(ok).is_ok(), "{ok}");
        }
        for bad in ["", ".", "e3", "1e", "1.2.3", "inf", "infinity", "nan", "NaN", "INF", "-INF", "0x10", "1_000", "1e999", "--1", "1 2", "١"] {
            assert!(double(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ranges() {
        assert!(positive("0").is_err());
        assert!(positive("1e-300").is_ok());
        assert!(unit("1.0000001").is_err());
        assert_eq!(duration("1000000"), Ok(1_000_000.0));
        assert!(duration("1000000.5").is_err());
        assert!(decibel("-121").is_err());
        assert_eq!(boolean(" 1 "), Ok(true));
        assert!(boolean("yes").is_err());
        assert_eq!(integer_in("+3", 0, 10), Ok(3));
        assert!(integer_in("99999999999999999999", 0, i64::MAX).is_err());
        assert!(integer_in("1.0", 0, 10).is_err());
        assert_eq!(unsigned_long("18446744073709551615"), Ok(u64::MAX));
        assert!(unsigned_long("-1").is_err());
    }

    #[test]
    fn seconds_are_checked() {
        assert_eq!(seconds("1"), Ok(Tick(TICKS_PER_SECOND)));
        assert_eq!(seconds("-0.5"), Ok(Tick(-TICKS_PER_SECOND / 2)));
        // 1e300 s would saturate the unchecked conversion; here it is an error
        assert!(seconds("1e300").is_err());
        assert!(seconds_to_tick(f64::NAN).is_none());
        assert!(seconds_to_tick(Tick::MAX.seconds() * 1.0001).is_none());
        assert!(seconds_to_tick(Tick::MAX.seconds() * 0.9999).is_some());
        // 1/1001 s lands on the nearest tick
        let t = seconds_to_tick(1.0 / 1001.0).unwrap_or(Tick::ZERO);
        assert!((t.0 - TICKS_PER_SECOND / 1001).abs() <= 1);
    }

    #[test]
    fn frame_rates() {
        assert_eq!(fps("30"), Ok(FrameRate::new(30, 1)));
        assert_eq!(fps("30000/1001"), Ok(FrameRate::new(30000, 1001)));
        assert_eq!(fps("50/2"), Ok(FrameRate::new(25, 1)));
        for bad in ["0", "030", "30/0", "30/", "/1", "29.97", "-30", "30/1/1", "1000001", "1/1000001", "99999999999999999999999"] {
            assert!(fps(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn lengths() {
        assert_eq!(length("12.5"), Ok(Length::px(12.5)));
        assert_eq!(length("1e2"), Ok(Length::px(100.0)));
        assert_eq!(length("-50%"), Ok(Length { value: -50.0, unit: LengthUnit::Percent }));
        assert_eq!(length("10vmin"), Ok(Length { value: 10.0, unit: LengthUnit::Vmin }));
        assert_eq!(length(".5vw"), Ok(Length { value: 0.5, unit: LengthUnit::Vw }));
        // relativeLength has no exponent and no '+'
        for bad in ["1e2%", "+5%", "%", "5 %", "5px", "5vm", "nan%", "inf"] {
            assert!(length(bad).is_err(), "{bad}");
        }
        assert!(positive_length("0").is_err());
        assert!(positive_length("0%").is_err());
        assert!(positive_length("-5vw").is_err());
        assert!(positive_length("0.0001%").is_ok());
        let l = Length { value: 50.0, unit: LengthUnit::Vmax };
        assert_eq!(l.resolve(10.0, 1920.0, 1080.0), 960.0);
        assert_eq!(Length { value: 25.0, unit: LengthUnit::Percent }.resolve(200.0, 1.0, 1.0), 50.0);
    }

    #[test]
    fn colours_and_paints() {
        assert_eq!(color("#FF0000"), Ok(Paint::Color(Rgba([1.0, 0.0, 0.0, 1.0]))));
        assert_eq!(color("#00000000"), Ok(Paint::Color(Rgba([0.0; 4]))));
        assert_eq!(color("1,0.5,0"), Ok(Paint::Color(Rgba([1.0, 0.5, 0.0, 1.0]))));
        assert_eq!(color(".25,0,1.000,0"), Ok(Paint::Color(Rgba([0.25, 0.0, 1.0, 0.0]))));
        assert_eq!(color("var(--brand-1)"), Ok(Paint::Var("brand-1".into())));
        for bad in ["#F00", "#FFF0", "#GG0000", "1.5,0,0", "1.1,0,0", "0,0", "0,0,0,0,0", "00,0,0", "1,0,0,", "var(--)", "var(--a b)", "red", "#FF0000FF00"] {
            assert!(color(bad).is_err(), "{bad}");
        }
        assert_eq!(paint("url(#grad.1)"), Ok(Paint::Ref("grad.1".into())));
        assert!(paint("url(#1grad)").is_err());
        assert!(paint("url(grad)").is_err());
        assert_eq!(paint("#000000FF"), Ok(Paint::Color(Rgba::BLACK)));
    }

    #[test]
    fn points_lists_timecodes() {
        assert_eq!(point("1,-2.5e1"), Ok((1.0, -25.0)));
        assert_eq!(point(".5,0"), Ok((0.5, 0.0)));
        for bad in ["1", "1,", "+1,2", "1.,2", "1,2,3", "a,b", "1 ,2"] {
            assert!(point(bad).is_err(), "{bad}");
        }
        assert_eq!(number_list(" 1 2\t3\n"), Ok(vec![1.0, 2.0, 3.0]));
        assert_eq!(number_list(""), Ok(vec![]));
        assert!(number_list("1 x").is_err());
        assert_eq!(point_list("0,0 1,1"), Ok(vec![(0.0, 0.0), (1.0, 1.0)]));
        assert_eq!(timecode("01:02:03;04"), Ok((1, 2, 3, 4, true)));
        assert!(timecode("01:60:00:00").is_err());
        assert!(timecode("1:02:03:04").is_err());
        assert!(timecode("01:02:03.04").is_err());
        assert!(sha256_hex(&"a".repeat(64)).is_ok());
        assert!(sha256_hex(&"A".repeat(64)).is_err());
    }

    #[test]
    fn hostile_lists_are_capped() {
        let huge = "0 ".repeat(MAX_LIST_ITEMS + 1);
        assert!(number_list(&huge).is_err());
        let fine = "0 ".repeat(MAX_LIST_ITEMS);
        assert_eq!(number_list(&fine).map(|v| v.len()), Ok(MAX_LIST_ITEMS));
    }

    #[test]
    fn hostile_multibyte_never_panics() {
        // every parser slices strings; none may split a UTF-8 sequence
        for s in ["#é0000", "#ééé", "é,é", "1é%", "var(--é)", "url(#é)", "é:é:é:é", "30/é", "1eé", "ééééééééééé"] {
            let _ = (double(s), length(s), color(s), paint(s), point(s), timecode(s), fps(s), sha256_hex(s));
        }
    }
}
