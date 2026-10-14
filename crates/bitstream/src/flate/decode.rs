//! Bounded decoder adapted from FilmCraft's OpenEXR inflater.
use super::{CL_ORDER, DIST_BASE, DIST_EXTRA, LEN_BASE, LEN_EXTRA};
type Error = &'static str;

pub(super) fn inflate(src: &[u8], limit: usize) -> Result<(Vec<u8>, usize), Error> {
    let mut bits = Bits { src, pos: 0, buf: 0, cnt: 0 };
    let mut out = Vec::new();
    let mut fixed: Option<(Huffman, Huffman)> = None;
    loop {
        let last = bits.take(1)? == 1;
        match bits.take(2)? {
            0 => stored(&mut bits, &mut out, limit)?,
            1 => {
                let (lit, dist) = match &fixed {
                    Some(t) => t,
                    None => fixed.insert(fixed_tables()?),
                };
                codes(&mut bits, &mut out, limit, lit, dist)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut bits)?;
                codes(&mut bits, &mut out, limit, &lit, &dist)?;
            }
            _ => return Err("invalid deflate block type"),
        }
        if last {
            // Refill reads ahead. Count only bytes containing consumed bits, including padding.
            return Ok((out, bits.pos - (bits.cnt / 8) as usize));
        }
    }
}

fn reserve(out: &mut Vec<u8>, additional: usize, limit: usize) -> Result<(), Error> {
    let end = out.len().checked_add(additional).filter(|&n| n <= limit).ok_or("deflate output exceeds the expected size")?;
    if end > out.capacity() {
        let capacity = end.max(out.capacity().saturating_mul(2)).max(8192).min(limit);
        out.try_reserve_exact(capacity - out.len()).map_err(|_| "output too large to allocate")?;
    }
    Ok(())
}

/// LSB-first bit reader.
struct Bits<'a> {
    src: &'a [u8],
    pos: usize,
    buf: u64,
    cnt: u32,
}

impl Bits<'_> {
    fn refill(&mut self) {
        while self.cnt <= 56 {
            let Some(&b) = self.src.get(self.pos) else { return };
            self.buf |= u64::from(b) << self.cnt;
            self.pos += 1;
            self.cnt += 8;
        }
    }
    /// The next `n` (≤ 32) bits without consuming them; zeros past the end of the input.
    fn peek(&mut self, n: u32) -> u32 {
        self.refill();
        (self.buf & ((1u64 << n) - 1)) as u32
    }
    fn consume(&mut self, n: u32) -> Result<(), Error> {
        if n > self.cnt {
            return Err("truncated deflate stream");
        }
        self.buf >>= n;
        self.cnt -= n;
        Ok(())
    }
    fn take(&mut self, n: u32) -> Result<u32, Error> {
        let v = self.peek(n);
        self.consume(n)?;
        Ok(v)
    }
}

/// A stored (uncompressed) block.
fn stored(bits: &mut Bits, out: &mut Vec<u8>, limit: usize) -> Result<(), Error> {
    bits.consume(bits.cnt % 8)?;
    let len = bits.take(16)?;
    if len != !bits.take(16)? & 0xFFFF {
        return Err("stored block length mismatch");
    }
    reserve(out, len as usize, limit)?;
    for _ in 0..len {
        out.push(bits.take(8)? as u8);
    }
    Ok(())
}

/// Codes of up to this many bits decode with one table lookup; longer ones walk the canonical code.
const FAST_BITS: u32 = 10;
const MAX_BITS: usize = 15;

/// A canonical Huffman code (RFC 1951 §3.2.2).
struct Huffman {
    /// Indexed by the next `FAST_BITS` input bits: `symbol << 4 | length`, 0 when the code is longer
    /// (or unassigned).
    fast: Vec<u16>,
    /// Number of codes of each length.
    count: [u16; MAX_BITS + 1],
    /// Symbols ordered by code (length, then symbol value).
    symbols: Vec<u16>,
}

impl Huffman {
    /// Build from per-symbol code lengths (0 = unused). Over-subscribed lengths are an error;
    /// only one-bit singleton alphabets may be incomplete.
    fn new(lengths: &[u8], singleton: bool) -> Result<Self, Error> {
        let mut count = [0u16; MAX_BITS + 1];
        for &l in lengths {
            let c = count.get_mut(usize::from(l)).ok_or("code length over 15")?;
            *c += 1;
        }
        *count.first_mut().ok_or("invalid Huffman code")? = 0;
        let mut left: i32 = 1;
        for &c in count.iter().skip(1) {
            left = (left << 1) - i32::from(c);
            if left < 0 {
                return Err("over-subscribed Huffman code");
            }
        }
        let used: u16 = count.iter().sum();
        if left != 0 && !(singleton && used == 1 && count.get(1) == Some(&1)) {
            return Err("incomplete Huffman code");
        }
        // first index into `symbols` and first code of each length
        let mut offs = [0usize; MAX_BITS + 2];
        let mut next = [0u32; MAX_BITS + 2];
        let mut code = 0u32;
        for len in 1..=MAX_BITS {
            *offs.get_mut(len + 1).ok_or("invalid Huffman code")? =
                *offs.get(len).ok_or("invalid Huffman code")? + usize::from(*count.get(len).ok_or("invalid Huffman code")?);
            code = (code + u32::from(*count.get(len - 1).ok_or("invalid Huffman code")?)) << 1;
            *next.get_mut(len).ok_or("invalid Huffman code")? = code;
        }
        let mut symbols = vec![0u16; *offs.get(MAX_BITS + 1).ok_or("invalid Huffman code")?];
        let mut fast = vec![0u16; 1 << FAST_BITS];
        for (sym, &l) in lengths.iter().enumerate() {
            let len = usize::from(l);
            if len == 0 {
                continue;
            }
            let offset = offs.get_mut(len).ok_or("invalid Huffman code")?;
            *symbols.get_mut(*offset).ok_or("invalid Huffman code")? = sym as u16;
            *offset += 1;
            let next_code = next.get_mut(len).ok_or("invalid Huffman code")?;
            let c = *next_code;
            *next_code += 1;
            if len as u32 <= FAST_BITS {
                // codes are sent MSB first; the reader sees them bit-reversed
                let rev = c.reverse_bits() >> (32 - len as u32);
                let entry = (sym as u16) << 4 | len as u16;
                let mut i = rev as usize;
                while let Some(e) = fast.get_mut(i) {
                    *e = entry;
                    i += 1 << len;
                }
            }
        }
        Ok(Self { fast, count, symbols })
    }

    fn decode(&self, bits: &mut Bits) -> Result<u16, Error> {
        let peek = bits.peek(MAX_BITS as u32);
        let e = self.fast.get((peek & ((1 << FAST_BITS) - 1)) as usize).copied().unwrap_or(0);
        if e != 0 {
            bits.consume(u32::from(e & 15))?;
            return Ok(e >> 4);
        }
        // canonical walk: codes of each length are consecutive, starting at `first`
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAX_BITS {
            code |= ((peek >> (len - 1)) & 1) as i32;
            let count = i32::from(*self.count.get(len).ok_or("invalid Huffman code")?);
            if code - first < count {
                bits.consume(len as u32)?;
                return self.symbols.get((index + code - first) as usize).copied().ok_or("invalid Huffman code");
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err("invalid Huffman code")
    }
}

fn fixed_tables() -> Result<(Huffman, Huffman), Error> {
    let mut lit = [8u8; 288];
    lit.get_mut(144..256).ok_or("invalid Huffman code")?.fill(9);
    lit.get_mut(256..280).ok_or("invalid Huffman code")?.fill(7);
    Ok((Huffman::new(&lit, false)?, Huffman::new(&[5; 32], false)?))
}

fn dynamic_tables(bits: &mut Bits) -> Result<(Huffman, Huffman), Error> {
    let nlit = bits.take(5)? as usize + 257;
    let ndist = bits.take(5)? as usize + 1;
    let ncl = bits.take(4)? as usize + 4;
    if nlit > 286 || ndist > 32 {
        return Err("too many deflate codes");
    }
    let mut cl = [0u8; 19];
    for &i in CL_ORDER.iter().take(ncl) {
        *cl.get_mut(i).ok_or("invalid code length symbol")? = bits.take(3)? as u8;
    }
    let clh = Huffman::new(&cl, false)?;
    let mut lengths = vec![0u8; nlit + ndist];
    let mut i = 0;
    while i < lengths.len() {
        let sym = clh.decode(bits)?;
        let (value, repeat) = match sym {
            0..=15 => (sym as u8, 1),
            16 => {
                let prev = *lengths.get(i.wrapping_sub(1)).ok_or("repeat with no previous length")?;
                (prev, 3 + bits.take(2)? as usize)
            }
            17 => (0, 3 + bits.take(3)? as usize),
            18 => (0, 11 + bits.take(7)? as usize),
            _ => return Err("invalid code length symbol"),
        };
        let run = lengths.get_mut(i..i + repeat).ok_or("code lengths overflow")?;
        run.fill(value);
        i += repeat;
    }
    let lit = lengths.get(..nlit).ok_or("code lengths overflow")?;
    let dist = lengths.get(nlit..).ok_or("code lengths overflow")?;
    if lit.get(256).copied().unwrap_or(0) == 0 {
        return Err("no end-of-block code");
    }
    let distance = if dist.iter().all(|&length| length == 0) {
        Huffman { fast: vec![0; 1 << FAST_BITS], count: [0; MAX_BITS + 1], symbols: Vec::new() }
    } else {
        Huffman::new(dist, true)?
    };
    Ok((Huffman::new(lit, true)?, distance))
}

/// The literal / length and distance codes of one compressed block.
fn codes(bits: &mut Bits, out: &mut Vec<u8>, limit: usize, lit: &Huffman, dist: &Huffman) -> Result<(), Error> {
    loop {
        let sym = lit.decode(bits)?;
        if sym < 256 {
            reserve(out, 1, limit)?;
            out.push(sym as u8);
            continue;
        }
        if sym == 256 {
            return Ok(());
        }
        let k = usize::from(sym - 257);
        let (&base, &extra) = LEN_BASE.get(k).zip(LEN_EXTRA.get(k)).ok_or("invalid length code")?;
        let len = usize::from(base) + bits.take(u32::from(extra))? as usize;
        let d = usize::from(dist.decode(bits)?);
        let (&base, &extra) = DIST_BASE.get(d).zip(DIST_EXTRA.get(d)).ok_or("invalid distance code")?;
        let distance = usize::from(base) + bits.take(u32::from(extra))? as usize;
        if distance > out.len() {
            return Err("distance before the start of the output");
        }
        reserve(out, len, limit)?;
        let from = out.len() - distance;
        if distance >= len {
            out.extend_from_within(from..from + len);
        } else {
            // overlapping copy: repeats the last `distance` bytes
            for j in 0..len {
                let b = out.get(from + j).copied().ok_or("invalid distance code")?;
                out.push(b);
            }
        }
    }
}
