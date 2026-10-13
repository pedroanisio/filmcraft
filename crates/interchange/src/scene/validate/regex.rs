//! XSD 1.0 regular expressions (`xs:pattern`), compiled to a Pike VM.
//!
//! Supports what the schema uses and nothing that would need backtracking: alternation, groups,
//! `? * + {n} {n,} {n,m}`, `.` (any character except `\n` and `\r`), character classes with ranges
//! and negation, and the escapes `\s \S \d \D \n \r \t` plus escaped metacharacters. Matching is
//! implicitly anchored at both ends, as XSD requires, and runs in O(text × program) time, so no
//! input can make it slow. Anything else (`\p{..}`, class subtraction, `\w`) is a compile error:
//! the schema-table test fails rather than a pattern silently matching the wrong language.

/// Longest pattern source accepted.
const MAX_SOURCE: usize = 1024;
/// Largest compiled program (counted repetitions expand in place).
const MAX_INSTS: usize = 8192;
/// Deepest group nesting.
const MAX_NESTING: usize = 32;
/// Largest `{n,m}` bound.
const MAX_REPEAT: u32 = 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Set {
    ranges: Vec<(char, char)>,
    negated: bool,
}

impl Set {
    fn of(ranges: &[(char, char)], negated: bool) -> Set {
        Set { ranges: ranges.to_vec(), negated }
    }

    fn matches(&self, c: char) -> bool {
        self.ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi) != self.negated
    }
}

const SPACE: &[(char, char)] = &[(' ', ' '), ('\t', '\t'), ('\n', '\n'), ('\r', '\r')];
const DIGIT: &[(char, char)] = &[('0', '9')];
const LINE_BREAK: &[(char, char)] = &[('\n', '\n'), ('\r', '\r')];

#[derive(Clone, Debug)]
enum Ast {
    Set(Set),
    Concat(Vec<Ast>),
    Alt(Vec<Ast>),
    Repeat { inner: Box<Ast>, min: u32, max: Option<u32> },
}

#[derive(Clone, Debug)]
enum Inst {
    Set(Set),
    Split(usize, usize),
    Jmp(usize),
    Match,
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Regex {
    prog: Vec<Inst>,
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    depth: usize,
}

impl Parser<'_> {
    fn alt(&mut self) -> Result<Ast, String> {
        let mut alts = vec![self.concat()?];
        while self.chars.peek() == Some(&'|') {
            self.chars.next();
            alts.push(self.concat()?);
        }
        Ok(if alts.len() == 1 { alts.remove(0) } else { Ast::Alt(alts) })
    }

    fn concat(&mut self) -> Result<Ast, String> {
        let mut items = Vec::new();
        while let Some(&c) = self.chars.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            items.push(self.quantified(atom)?);
        }
        Ok(Ast::Concat(items))
    }

    fn quantified(&mut self, atom: Ast) -> Result<Ast, String> {
        let (min, max) = match self.chars.peek() {
            Some('?') => (0, Some(1)),
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('{') => {
                self.chars.next();
                let min = self.number()?;
                let max = match self.chars.next() {
                    Some('}') => return Ok(Ast::Repeat { inner: Box::new(atom), min, max: Some(min) }),
                    Some(',') if self.chars.peek() == Some(&'}') => None,
                    Some(',') => Some(self.number()?),
                    _ => return Err("malformed {n,m} quantifier".into()),
                };
                if self.chars.next() != Some('}') {
                    return Err("malformed {n,m} quantifier".into());
                }
                if max.is_some_and(|m| m < min) {
                    return Err("{n,m} with m < n".into());
                }
                if self.chars.peek().is_some_and(|c| matches!(c, '?' | '*' | '+' | '{')) {
                    return Err("stacked quantifiers".into());
                }
                return Ok(Ast::Repeat { inner: Box::new(atom), min, max });
            }
            _ => return Ok(atom),
        };
        self.chars.next();
        if self.chars.peek().is_some_and(|c| matches!(c, '?' | '*' | '+' | '{')) {
            return Err("stacked quantifiers".into());
        }
        Ok(Ast::Repeat { inner: Box::new(atom), min, max })
    }

    fn number(&mut self) -> Result<u32, String> {
        let mut n: u32 = 0;
        let mut any = false;
        while let Some(d) = self.chars.peek().and_then(|c| c.to_digit(10)) {
            self.chars.next();
            any = true;
            n = n.checked_mul(10).and_then(|n| n.checked_add(d)).ok_or("repeat bound too large")?;
        }
        if !any {
            return Err("missing repeat bound".into());
        }
        if n > MAX_REPEAT {
            return Err("repeat bound too large".into());
        }
        Ok(n)
    }

    fn atom(&mut self) -> Result<Ast, String> {
        match self.chars.next() {
            Some('(') => {
                self.depth += 1;
                if self.depth > MAX_NESTING {
                    return Err("groups nested too deeply".into());
                }
                let inner = self.alt()?;
                if self.chars.next() != Some(')') {
                    return Err("unclosed group".into());
                }
                self.depth -= 1;
                Ok(inner)
            }
            Some('[') => self.class(),
            Some('.') => Ok(Ast::Set(Set::of(LINE_BREAK, true))),
            Some('\\') => self.escape().map(Ast::Set),
            Some(c @ ('?' | '*' | '+' | '{' | '}' | ']' | ')' | '|')) => Err(format!("unexpected {c:?}")),
            Some(c) => Ok(Ast::Set(Set::of(&[(c, c)], false))),
            None => Err("unexpected end of pattern".into()),
        }
    }

    /// The character after a backslash, outside or inside a class.
    fn escape(&mut self) -> Result<Set, String> {
        let single = |c: char| Set::of(&[(c, c)], false);
        match self.chars.next() {
            Some('s') => Ok(Set::of(SPACE, false)),
            Some('S') => Ok(Set::of(SPACE, true)),
            Some('d') => Ok(Set::of(DIGIT, false)),
            Some('D') => Ok(Set::of(DIGIT, true)),
            Some('n') => Ok(single('\n')),
            Some('r') => Ok(single('\r')),
            Some('t') => Ok(single('\t')),
            Some(c @ ('\\' | '|' | '.' | '-' | '^' | '?' | '*' | '+' | '{' | '}' | '(' | ')' | '[' | ']')) => Ok(single(c)),
            Some(c) => Err(format!("unsupported escape \\{c}")),
            None => Err("trailing backslash".into()),
        }
    }

    fn class(&mut self) -> Result<Ast, String> {
        let negated = self.chars.peek() == Some(&'^');
        if negated {
            self.chars.next();
        }
        let mut ranges = Vec::new();
        let mut first = true;
        loop {
            let c = self.chars.next().ok_or("unclosed character class")?;
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = match c {
                '\\' => {
                    let set = self.escape()?;
                    if set.negated || set.ranges.len() != 1 || set.ranges.first().is_none_or(|r| r.0 != r.1) {
                        // a multi-character escape: only positive ones can join a class
                        if set.negated {
                            return Err("negated escape inside a class".into());
                        }
                        ranges.extend(set.ranges);
                        continue;
                    }
                    set.ranges.first().map(|r| r.0).ok_or("empty escape")?
                }
                '[' => return Err("nested class or class subtraction".into()),
                '-' if self.chars.peek() == Some(&'[') => return Err("class subtraction".into()),
                c => c,
            };
            if self.chars.peek() == Some(&'-') {
                let mut ahead = self.chars.clone();
                ahead.next();
                if ahead.peek().is_some_and(|&n| n != ']' && n != '[') {
                    self.chars.next();
                    let hi = match self.chars.next() {
                        Some('\\') => {
                            let set = self.escape()?;
                            match set.ranges.as_slice() {
                                [(a, b)] if a == b && !set.negated => *a,
                                _ => return Err("range ends in a multi-character escape".into()),
                            }
                        }
                        Some(h) => h,
                        None => return Err("unclosed character class".into()),
                    };
                    if hi < lo {
                        return Err(format!("reversed range {lo:?}-{hi:?}"));
                    }
                    ranges.push((lo, hi));
                    continue;
                }
            }
            ranges.push((lo, lo));
        }
        Ok(Ast::Set(Set { ranges, negated }))
    }
}

struct Compiler {
    prog: Vec<Inst>,
}

impl Compiler {
    fn push(&mut self, i: Inst) -> Result<usize, String> {
        if self.prog.len() >= MAX_INSTS {
            return Err("pattern compiles too large".into());
        }
        self.prog.push(i);
        Ok(self.prog.len() - 1)
    }

    fn patch(&mut self, at: usize, i: Inst) {
        if let Some(slot) = self.prog.get_mut(at) {
            *slot = i;
        }
    }

    fn emit(&mut self, a: &Ast) -> Result<(), String> {
        match a {
            Ast::Set(s) => {
                self.push(Inst::Set(s.clone()))?;
            }
            Ast::Concat(items) => {
                for it in items {
                    self.emit(it)?;
                }
            }
            Ast::Alt(alts) => {
                let mut jumps = Vec::new();
                let last = alts.len().saturating_sub(1);
                for (k, alt) in alts.iter().enumerate() {
                    if k < last {
                        let split = self.push(Inst::Match)?;
                        self.emit(alt)?;
                        jumps.push(self.push(Inst::Match)?);
                        let next = self.prog.len();
                        self.patch(split, Inst::Split(split + 1, next));
                    } else {
                        self.emit(alt)?;
                    }
                }
                let end = self.prog.len();
                for j in jumps {
                    self.patch(j, Inst::Jmp(end));
                }
            }
            Ast::Repeat { inner, min, max } => {
                for _ in 0..*min {
                    self.emit(inner)?;
                }
                match max {
                    None => {
                        let split = self.push(Inst::Match)?;
                        self.emit(inner)?;
                        self.push(Inst::Jmp(split))?;
                        let end = self.prog.len();
                        self.patch(split, Inst::Split(split + 1, end));
                    }
                    Some(max) => {
                        let mut splits = Vec::new();
                        for _ in *min..*max {
                            splits.push(self.push(Inst::Match)?);
                            self.emit(inner)?;
                        }
                        let end = self.prog.len();
                        for s in splits {
                            self.patch(s, Inst::Split(s + 1, end));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Threads of the Pike VM: program counters in priority order plus a membership mark per pc.
struct Threads {
    list: Vec<usize>,
    seen: Vec<bool>,
    stack: Vec<usize>,
}

impl Threads {
    fn new(n: usize) -> Threads {
        Threads { list: Vec::with_capacity(n), seen: vec![false; n], stack: Vec::new() }
    }

    fn clear(&mut self) {
        for pc in self.list.drain(..) {
            if let Some(s) = self.seen.get_mut(pc) {
                *s = false;
            }
        }
    }

    /// Adds `pc` and everything reachable from it through jumps and splits.
    fn add(&mut self, prog: &[Inst], pc: usize) {
        self.stack.push(pc);
        while let Some(pc) = self.stack.pop() {
            let Some(seen) = self.seen.get_mut(pc) else { continue };
            if *seen {
                continue;
            }
            *seen = true;
            match prog.get(pc) {
                Some(Inst::Jmp(t)) => self.stack.push(*t),
                Some(Inst::Split(a, b)) => {
                    self.stack.push(*b);
                    self.stack.push(*a);
                }
                Some(_) => {}
                None => continue,
            }
            // every visited pc is recorded so `clear` can reset its mark; only sets and Match act
            self.list.push(pc);
        }
    }
}

impl Regex {
    /// Compiles an XSD pattern.
    pub fn new(source: &str) -> Result<Regex, String> {
        if source.len() > MAX_SOURCE {
            return Err("pattern too long".into());
        }
        let mut p = Parser { chars: source.chars().peekable(), depth: 0 };
        let ast = p.alt()?;
        if let Some(c) = p.chars.next() {
            return Err(format!("unexpected {c:?}"));
        }
        let mut c = Compiler { prog: Vec::new() };
        c.emit(&ast)?;
        c.push(Inst::Match)?;
        Ok(Regex { prog: c.prog })
    }

    /// Whether the whole of `text` matches.
    pub fn is_match(&self, text: &str) -> bool {
        let n = self.prog.len();
        let mut cur = Threads::new(n);
        let mut next = Threads::new(n);
        cur.add(&self.prog, 0);
        for c in text.chars() {
            if cur.list.is_empty() {
                return false;
            }
            next.clear();
            for &pc in &cur.list {
                if let Some(Inst::Set(s)) = self.prog.get(pc)
                    && s.matches(c)
                {
                    next.add(&self.prog, pc + 1);
                }
            }
            std::mem::swap(&mut cur, &mut next);
        }
        cur.list.iter().any(|&pc| matches!(self.prog.get(pc), Some(Inst::Match)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, t: &str) -> bool {
        Regex::new(p).unwrap().is_match(t)
    }

    #[test]
    fn basics() {
        assert!(m("abc", "abc"));
        assert!(!m("abc", "abcd"));
        assert!(!m("abc", "ab"));
        assert!(m("a|bc", "bc"));
        assert!(m("(ab)+", "ababab"));
        assert!(!m("(ab)+", ""));
        assert!(m("a{2,3}", "aaa"));
        assert!(!m("a{2,3}", "aaaa"));
        assert!(m("a{2,}", "aaaaaaa"));
        assert!(m("[0-9a-f]{4}", "0fa9"));
        assert!(!m("[0-9a-f]{4}", "0fg9"));
        assert!(m("[^0-9]x", "ax"));
        assert!(m(".*", "anything at all"));
        assert!(!m(".", "\n"));
        assert!(m(r"\s*x\s*", " \t x\n"));
        assert!(m(r"[A-Za-z0-9_\-]+", "a-b_C"));
        assert!(m(r"[\-+]?1", "-1"));
        assert!(m("[a-]", "-"));
        assert!(m(r"var\(--x\)", "var(--x)"));
        assert!(m("", ""));
        assert!(m("a?", ""));
    }

    #[test]
    fn rejects_unsupported() {
        for p in [r"\p{L}", r"[a-z-[aeiou]]", r"\w", "(a", "a)", "[a", "a**", "a{3,2}", "a{1001}", "[z-a]"] {
            assert!(Regex::new(p).is_err(), "{p}");
        }
    }

    #[test]
    fn linear_on_hostile_input() {
        // classic catastrophic-backtracking shape; a Pike VM stays linear
        let r = Regex::new("(a|a)*(a*)*b").unwrap();
        let text = "a".repeat(50_000);
        assert!(!r.is_match(&text));
        let r = Regex::new("(x+x+)+y").unwrap();
        assert!(!r.is_match(&"x".repeat(50_000)));
    }

    #[test]
    fn caps_program_size() {
        assert!(Regex::new("((a{1000}){1000})").is_err());
    }
}
