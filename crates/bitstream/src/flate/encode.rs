//! A small DEFLATE encoder (RFC 1951) in a zlib wrapper (RFC 1950), written from the RFCs for the
//! ZIP-compressed OpenEXR chunks.
//!
//! Greedy LZ77 over a 32 KB window (hash chains of bounded length), then each block of tokens is
//! written as whichever of stored, fixed-Huffman or dynamic-Huffman is smallest. Huffman code
//! lengths are limited (15 bits, 7 for the code-length code) by halving the symbol frequencies
//! until the tree fits. Every code has at least two symbols, so all codes are complete. The
//! output favours simplicity and correctness over ratio; worst-case expansion is the stored-block
//! overhead (5 bytes per 64 KB).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

const WINDOW: usize = 32 * 1024;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
/// Hash chain candidates tried per position.
const MAX_CHAIN: usize = 48;
/// Tokens per block (a new set of Huffman codes after this many).
const BLOCK_TOKENS: usize = 1 << 15;
const MAX_STORED: usize = 65_535;

use super::{CL_ORDER, DIST_BASE, DIST_EXTRA, LEN_BASE, LEN_EXTRA};

pub(super) fn encode(data: &[u8], wrapped: bool) -> Vec<u8> {
    let mut w = Bits { out: Vec::new(), acc: 0, n: 0 };
    if wrapped {
        w.out.extend_from_slice(&[0x78, 0x9c]);
    }
    if deflate(data, &mut w).is_none() {
        // A failed internal lookup can always be represented losslessly with stored blocks.
        w.out.clear();
        w.acc = 0;
        w.n = 0;
        if wrapped {
            w.out.extend_from_slice(&[0x78, 0x9c]);
        }
        stored(&mut w, data, true);
    }
    w.align();
    if wrapped {
        w.out.extend_from_slice(&adler32(data).to_be_bytes());
    }
    w.out
}

/// Adler-32 (RFC 1950 §9); 5552 bytes is the longest run that cannot overflow the sums.
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for c in data.chunks(5552) {
        for &x in c {
            a += u32::from(x);
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    (b << 16) | a
}

/// LSB-first bit writer.
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    /// The low `n` bits of `v` (n ≤ 32).
    fn put(&mut self, v: u32, n: u32) {
        let mask = (1_u64 << n) - 1;
        self.acc |= (u64::from(v) & mask) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    fn align(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.acc = 0;
        self.n = 0;
    }
}

#[derive(Clone, Copy)]
enum Tok {
    Lit(u8),
    Match { len: u16, dist: u16 },
}

/// Index into [`LEN_BASE`] of a match length (3..=258).
fn len_code(len: u16) -> usize {
    LEN_BASE.partition_point(|&b| b <= len).saturating_sub(1)
}

/// Index into [`DIST_BASE`] of a distance (1..=32768).
fn dist_code(dist: u16) -> usize {
    DIST_BASE.partition_point(|&b| b <= dist).saturating_sub(1)
}

/// LZ77 match finder: hash heads and chains over the window.
struct Matcher<'a> {
    data: &'a [u8],
    head: Vec<u32>,
    prev: Vec<u32>,
    bits: u32,
}

impl<'a> Matcher<'a> {
    fn new(data: &'a [u8]) -> Self {
        // tables sized to the input, so short chunks (single scan lines) stay cheap
        let span = data.len().clamp(256, WINDOW).next_power_of_two();
        let bits = span.trailing_zeros().clamp(8, 15);
        Matcher { data, head: vec![0; 1 << bits], prev: vec![0; span], bits }
    }

    fn hash(&self, p: usize) -> Option<usize> {
        let b = self.data.get(p..p.checked_add(MIN_MATCH)?)?;
        let [a, b, c] = b else { return None };
        let v = (u32::from(*a) << 16) | (u32::from(*b) << 8) | u32::from(*c);
        Some((v.wrapping_mul(0x9E37_79B1) >> (32 - self.bits)) as usize)
    }

    /// Record position `p` (stored as `p + 1`; 0 = empty).
    fn insert(&mut self, p: usize) -> Option<()> {
        let Some(h) = self.hash(p) else { return Some(()) };
        let mask = self.prev.len() - 1;
        *self.prev.get_mut(p & mask)? = *self.head.get(h)?;
        *self.head.get_mut(h)? = u32::try_from(p).ok()?.checked_add(1)?;
        Some(())
    }

    /// The longest earlier match at `p`: (length, distance), length 0 if none.
    fn longest(&self, p: usize) -> (usize, usize) {
        let Some(h) = self.hash(p) else { return (0, 0) };
        let max = (self.data.len() - p).min(MAX_MATCH);
        let Some(here) = self.data.get(p..p.saturating_add(max)) else { return (0, 0) };
        let mask = self.prev.len() - 1;
        let (mut best, mut best_dist) = (0, 0);
        let mut cand = self.head.get(h).copied().unwrap_or(0) as usize;
        for _ in 0..MAX_CHAIN {
            let Some(c) = cand.checked_sub(1) else { break };
            if c >= p || p - c > WINDOW {
                break;
            }
            let l = self.data.get(c..).unwrap_or_default().iter().zip(here).take_while(|(a, b)| a == b).count();
            if l > best {
                (best, best_dist) = (l, p - c);
                if l == max {
                    break;
                }
            }
            let next = self.prev.get(c & mask).copied().unwrap_or(0) as usize;
            // chains only go back; anything else is a stale slot
            if next == 0 || next > c {
                break;
            }
            cand = next;
        }
        (best, best_dist)
    }
}

/// The deflate blocks of `data`, the last one final.
fn deflate(data: &[u8], w: &mut Bits) -> Option<()> {
    if data.is_empty() {
        // a final fixed-Huffman block holding only end-of-block (code 0000000)
        w.put(1, 1);
        w.put(1, 2);
        w.put(0, 7);
        return Some(());
    }
    // positions are kept in u32; longer inputs (never an OpenEXR chunk) go out stored
    let mut m = (data.len() < u32::MAX as usize).then(|| Matcher::new(data));
    let mut toks = Vec::with_capacity(BLOCK_TOKENS.min(data.len()) + 1);
    let (mut start, mut i) = (0, 0);
    while i < data.len() {
        let (len, dist) = m.as_ref().map_or((0, 0), |m| m.longest(i));
        let step = if len >= MIN_MATCH {
            toks.push(Tok::Match { len: len as u16, dist: dist as u16 });
            len
        } else {
            toks.push(Tok::Lit(*data.get(i)?));
            1
        };
        if let Some(m) = m.as_mut() {
            for p in i..i + step {
                m.insert(p)?;
            }
        }
        i += step;
        if toks.len() >= BLOCK_TOKENS || i == data.len() {
            block(w, &toks, data.get(start..i)?, i == data.len())?;
            toks.clear();
            start = i;
        }
    }
    Some(())
}

/// Bits the tokens take with the given code lengths (EOB included).
fn data_bits(lf: &[u32], df: &[u32], ll: &[u8], dl: &[u8]) -> u64 {
    let mut n = 0_u64;
    for (s, (&f, &l)) in lf.iter().zip(ll).enumerate() {
        let extra = s.checked_sub(257).and_then(|k| LEN_EXTRA.get(k)).copied().unwrap_or(0);
        n += u64::from(f) * u64::from(l + extra);
    }
    for (d, (&f, &l)) in df.iter().zip(dl).enumerate() {
        n += u64::from(f) * u64::from(l + DIST_EXTRA.get(d).copied().unwrap_or(0));
    }
    n
}

/// One block (split into several stored blocks when stored is smallest and `raw` is long).
fn block(w: &mut Bits, toks: &[Tok], raw: &[u8], last: bool) -> Option<()> {
    let mut lf = [0_u32; 286];
    let mut df = [0_u32; 30];
    for t in toks {
        match *t {
            Tok::Lit(b) => *lf.get_mut(b as usize)? += 1,
            Tok::Match { len, dist } => {
                *lf.get_mut(257 + len_code(len))? += 1;
                *df.get_mut(dist_code(dist))? += 1;
            }
        }
    }
    *lf.get_mut(256)? += 1;

    // dynamic codes and their header
    let ll = lengths(&lf, 15)?;
    let dl = lengths(&df, 15)?;
    let hlit = ll.iter().rposition(|&l| l > 0).map_or(257, |p| (p + 1).max(257));
    let hdist = dl.iter().rposition(|&l| l > 0).map_or(1, |p| p + 1);
    let seq = rle_lengths(&[ll.get(..hlit)?, dl.get(..hdist)?].concat())?;
    let mut cf = [0_u32; 19];
    for &(s, _) in &seq {
        *cf.get_mut(s as usize)? += 1;
    }
    let cl = lengths(&cf, 7)?;
    let hclen = CL_ORDER.iter().rposition(|&s| cl.get(s).copied().unwrap_or(0) > 0).map_or(4, |p| (p + 1).max(4));
    let seq_bits: u64 = seq
        .iter()
        .map(|&(s, _)| u64::from(cl.get(s as usize).copied().unwrap_or(0)) + [2_u64, 3, 7].get((s as usize).wrapping_sub(16)).copied().unwrap_or(0))
        .sum();
    let dyn_bits = 3 + 14 + 3 * hclen as u64 + seq_bits + data_bits(&lf, &df, &ll, &dl);

    let (fll, fdl) = fixed_lengths()?;
    let fixed_bits = 3 + data_bits(&lf, &df, &fll, &fdl);
    // per stored piece: header, at most 7 padding bits, LEN and NLEN
    let pieces = raw.len().div_ceil(MAX_STORED).max(1) as u64;
    let stored_bits = pieces * (3 + 7 + 32) + 8 * raw.len() as u64;

    if stored_bits <= fixed_bits.min(dyn_bits) {
        stored(w, raw, last);
        return Some(());
    }
    w.put(u32::from(last), 1);
    if fixed_bits <= dyn_bits {
        w.put(1, 2);
        return write_tokens(w, toks, &fll, &fdl);
    }
    w.put(2, 2);
    w.put((hlit - 257) as u32, 5);
    w.put((hdist - 1) as u32, 5);
    w.put((hclen - 4) as u32, 4);
    for &s in CL_ORDER.iter().take(hclen) {
        w.put(u32::from(*cl.get(s)?), 3);
    }
    let cc = codes(&cl)?;
    for &(s, x) in &seq {
        w.put(u32::from(*cc.get(s as usize)?), u32::from(*cl.get(s as usize)?));
        match s {
            16 => w.put(u32::from(x), 2),
            17 => w.put(u32::from(x), 3),
            18 => w.put(u32::from(x), 7),
            _ => {}
        }
    }
    write_tokens(w, toks, &ll, &dl)
}

/// The fixed Huffman code lengths (RFC 1951 §3.2.6).
fn fixed_lengths() -> Option<([u8; 288], [u8; 30])> {
    let mut ll = [8_u8; 288];
    ll.get_mut(144..256)?.fill(9);
    ll.get_mut(256..280)?.fill(7);
    Some((ll, [5; 30]))
}

fn write_tokens(w: &mut Bits, toks: &[Tok], ll: &[u8], dl: &[u8]) -> Option<()> {
    let lc = codes(ll)?;
    let dc = codes(dl)?;
    let sym = |w: &mut Bits, s: usize| -> Option<()> {
        w.put(u32::from(*lc.get(s)?), u32::from(*ll.get(s)?));
        Some(())
    };
    for t in toks {
        match *t {
            Tok::Lit(b) => sym(w, b as usize)?,
            Tok::Match { len, dist } => {
                let k = len_code(len);
                sym(w, 257 + k)?;
                w.put(u32::from(len - *LEN_BASE.get(k)?), u32::from(*LEN_EXTRA.get(k)?));
                let d = dist_code(dist);
                w.put(u32::from(*dc.get(d)?), u32::from(*dl.get(d)?));
                w.put(u32::from(dist - *DIST_BASE.get(d)?), u32::from(*DIST_EXTRA.get(d)?));
            }
        }
    }
    sym(w, 256)
}

/// Canonical Huffman codes for `lens`, bit-reversed for the LSB-first writer.
fn codes(lens: &[u8]) -> Option<Vec<u16>> {
    let mut count = [0_u32; 16];
    for &l in lens {
        if (1..16).contains(&l) {
            *count.get_mut(l as usize)? += 1;
        }
    }
    let mut next = [0_u32; 16];
    let mut code = 0;
    for bits in 1..16 {
        code = (code + *count.get(bits - 1)?) << 1;
        *next.get_mut(bits)? = code;
    }
    lens.iter()
        .map(|&l| {
            if !(1..16).contains(&l) {
                return Some(0);
            }
            let entry = next.get_mut(l as usize)?;
            let c = *entry;
            *entry += 1;
            Some((c.reverse_bits() >> (32 - u32::from(l))) as u16)
        })
        .collect()
}

/// Code lengths for `freq`, none longer than `limit`; at least two symbols always get a code.
fn lengths(freq: &[u32], limit: u8) -> Option<Vec<u8>> {
    let mut f: Vec<u64> = freq.iter().map(|&x| u64::from(x)).collect();
    let mut used = f.iter().filter(|&&x| x > 0).count();
    for x in f.iter_mut() {
        if used >= 2 {
            break;
        }
        if *x == 0 {
            *x = 1;
            used += 1;
        }
    }
    // halving converges: once every used frequency is 1 the tree is balanced (depth ≤ 9 here)
    for _ in 0..66 {
        let l = huffman(&f)?;
        if l.iter().all(|&d| d <= limit) {
            return Some(l);
        }
        for x in f.iter_mut().filter(|x| **x > 0) {
            *x = (*x >> 1).max(1);
        }
    }
    huffman(&f)
}

/// Huffman tree depths of the symbols with non-zero frequency (0 for the rest).
fn huffman(f: &[u64]) -> Option<Vec<u8>> {
    let n = f.len();
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = f.iter().enumerate().filter(|(_, w)| **w > 0).map(|(i, &w)| Reverse((w, i))).collect();
    let mut parent = vec![usize::MAX; 2 * n];
    let mut next = n;
    while heap.len() > 1 {
        let (Some(Reverse((wa, a))), Some(Reverse((wb, b)))) = (heap.pop(), heap.pop()) else { break };
        *parent.get_mut(a)? = next;
        *parent.get_mut(b)? = next;
        heap.push(Reverse((wa + wb, next)));
        next += 1;
    }
    // parents always come after their children, so walk down from the root
    let mut depth = vec![0_u32; next];
    for i in (0..next).rev() {
        if let Some(&p) = parent.get(i).filter(|&&p| p < next) {
            *depth.get_mut(i)? = depth.get(p)?.saturating_add(1);
        }
    }
    Some(depth.get(..n)?.iter().map(|&d| d.min(u32::from(u8::MAX)) as u8).collect())
}

/// Code lengths run-length coded with the code-length alphabet: (symbol, extra-bits value).
fn rle_lengths(lens: &[u8]) -> Option<Vec<(u8, u8)>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lens.len() {
        let l = *lens.get(i)?;
        let run = lens.get(i..)?.iter().take_while(|&&x| x == l).count();
        let mut r = run;
        if l == 0 {
            while r >= 11 {
                let k = r.min(138);
                out.push((18, (k - 11) as u8));
                r -= k;
            }
            if r >= 3 {
                out.push((17, (r - 3) as u8));
                r = 0;
            }
        } else {
            out.push((l, 0));
            r -= 1;
            while r >= 3 {
                let k = r.min(6);
                out.push((16, (k - 3) as u8));
                r -= k;
            }
        }
        out.extend(std::iter::repeat_n((l, 0), r));
        i += run;
    }
    Some(out)
}

fn stored(w: &mut Bits, raw: &[u8], last: bool) {
    let n = raw.len().div_ceil(MAX_STORED);
    for (k, piece) in raw.chunks(MAX_STORED).enumerate() {
        w.put(u32::from(last && k + 1 == n), 1);
        w.put(0, 2);
        w.align();
        let len = piece.len() as u16;
        w.out.extend_from_slice(&len.to_le_bytes());
        w.out.extend_from_slice(&(!len).to_le_bytes());
        w.out.extend_from_slice(piece);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flate::{zlib_decode, zlib_encode as zlib};
    /// A deterministic pseudo-random byte stream.
    fn noise(n: usize, seed: u32) -> Vec<u8> {
        let mut s = seed | 1;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                s as u8
            })
            .collect()
    }

    #[test]
    fn round_trips() {
        let mut cases: Vec<Vec<u8>> = vec![vec![], vec![7], vec![1, 2], vec![0; 1], vec![0; 300_000], b"abcabcabcabcabcabd".to_vec()];
        cases.push(noise(100_000, 9)); // incompressible: stored blocks
        cases.push(noise(70_000, 3).iter().map(|b| b % 4).collect()); // few symbols: dynamic
        cases.push((0..200_000_u32).map(|i| ((i / 7) % 251) as u8).collect()); // long repeats
        // text-like: a small vocabulary repeated at varying distances
        let words: [&[u8]; 6] = [b"red ", b"green ", b"blue ", b"alpha ", b"half ", b"linear "];
        cases.push(noise(40_000, 5).iter().flat_map(|b| words[*b as usize % 6].iter().copied()).collect());
        // skewed frequencies: Huffman lengths beyond 15 before limiting
        let mut fib = Vec::new();
        let (mut a, mut b) = (1_usize, 1_usize);
        for s in 0..25_u8 {
            fib.extend(std::iter::repeat_n(s.wrapping_mul(37), a.min(200_000)));
            (a, b) = (b, a + b);
        }
        let r = noise(fib.len() * 4, 11);
        for i in (1..fib.len()).rev() {
            let j = u32::from_le_bytes(r[i * 4..i * 4 + 4].try_into().unwrap()) as usize % (i + 1);
            fib.swap(i, j);
        }
        cases.push(fib);
        for (k, data) in cases.iter().enumerate() {
            let z = zlib(data);
            assert_eq!(zlib_decode(&z, data.len()).unwrap(), *data, "case {k}");
            // worst case: stored overhead only
            assert!(z.len() <= data.len() + (data.len().div_ceil(BLOCK_TOKENS) + 1) * 5 + 6, "case {k}: {} > {}", z.len(), data.len());
        }
        // repetitive data actually compresses
        assert!(zlib(&cases[4]).len() < 2_000);
        assert!(zlib(&cases[8]).len() < 20_000);
    }

    #[test]
    fn length_limits_hold() {
        // Fibonacci frequencies give the deepest possible Huffman tree
        let mut f = vec![0_u32; 286];
        let (mut a, mut b) = (1_u32, 1_u32);
        for x in f.iter_mut().take(30) {
            *x = a;
            (a, b) = (b, a.saturating_add(b));
        }
        let l = lengths(&f, 15).unwrap();
        assert!(l.iter().all(|&d| d <= 15));
        // a complete code: Kraft sum exactly 1
        let kraft: f64 = l.iter().filter(|&&d| d > 0).map(|&d| 0.5_f64.powi(i32::from(d))).sum();
        assert!((kraft - 1.0).abs() < 1e-12);
        // one used symbol still gets a two-symbol code
        let l = lengths(&[0, 5, 0], 7).unwrap();
        assert_eq!(l.iter().filter(|&&d| d == 1).count(), 2);
    }

    #[test]
    fn adler_matches_rfc_example() {
        // the classic test vector for "Wikipedia"
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&[]), 1);
    }
}
