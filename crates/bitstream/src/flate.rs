//! Bounded RFC 1951 DEFLATE and RFC 1950 zlib streams.

/// Why a stream could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlateError {
    /// A malformed stream or an exceeded output budget.
    Invalid(&'static str),
}

impl std::fmt::Display for FlateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for FlateError {}

mod decode;
mod encode;
pub use encode::adler32;

/// Decode raw DEFLATE, rejecting trailing bytes and output beyond `limit`.
/// The budget is checked before growing the output; allocation failure is an error.
pub fn inflate(src: &[u8], limit: usize) -> Result<Vec<u8>, FlateError> {
    let (out, consumed) = inflate_with_consumed(src, limit)?;
    if consumed != src.len() {
        return Err(FlateError::Invalid("trailing deflate data"));
    }
    Ok(out)
}

/// Decode raw DEFLATE into exactly `expected` bytes, rejecting trailing bytes.
pub fn inflate_exact(src: &[u8], expected: usize) -> Result<Vec<u8>, FlateError> {
    let out = inflate(src, expected)?;
    if out.len() != expected {
        return Err(FlateError::Invalid("deflate stream ends early"));
    }
    Ok(out)
}

/// Decode one raw stream and report bytes consumed, including its final partial byte.
/// Trailing bytes are permitted and excluded from the count, even when read ahead internally.
/// `limit` caps the output. ZIP callers must compare both output length and consumed bytes
/// with their entry's declared sizes.
pub fn inflate_with_consumed(src: &[u8], limit: usize) -> Result<(Vec<u8>, usize), FlateError> {
    decode::inflate(src, limit).map_err(FlateError::Invalid)
}

/// Decode exactly `expected` bytes from zlib, checking its header and Adler-32 trailer.
/// Preset dictionaries and trailing bytes are rejected.
pub fn zlib_decode(src: &[u8], expected: usize) -> Result<Vec<u8>, FlateError> {
    let (&cmf, &flg) = match src {
        [a, b, ..] => (a, b),
        _ => return Err(FlateError::Invalid("truncated zlib header")),
    };
    if cmf & 0x0f != 8 || cmf >> 4 > 7 || (u16::from(cmf) << 8 | u16::from(flg)) % 31 != 0 {
        return Err(FlateError::Invalid("not a zlib stream"));
    }
    if flg & 0x20 != 0 {
        return Err(FlateError::Invalid("zlib preset dictionary"));
    }
    let payload = src.get(2..).ok_or(FlateError::Invalid("truncated zlib header"))?;
    let (out, consumed) = inflate_with_consumed(payload, expected)?;
    if out.len() != expected {
        return Err(FlateError::Invalid("zlib stream ends early"));
    }
    let trailer = payload.get(consumed..).ok_or(FlateError::Invalid("truncated zlib trailer"))?;
    let [a, b, c, d, rest @ ..] = trailer else {
        return Err(FlateError::Invalid("truncated zlib trailer"));
    };
    if !rest.is_empty() {
        return Err(FlateError::Invalid("trailing zlib data"));
    }
    if u32::from_be_bytes([*a, *b, *c, *d]) != adler32(&out) {
        return Err(FlateError::Invalid("zlib checksum mismatch"));
    }
    Ok(out)
}

/// Encode a raw DEFLATE stream (RFC 1951), using a bounded 32 KiB match window.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    encode::encode(data, false)
}

/// Encode a zlib stream (RFC 1950), byte-identical to FilmCraft's original EXR encoder.
pub fn zlib_encode(data: &[u8]) -> Vec<u8> {
    encode::encode(data, true)
}

const LEN_BASE: [u16; 29] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] =
    [1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

#[cfg(test)]
mod tests;
