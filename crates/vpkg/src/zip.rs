//! Bounded ZIP records; ZIP64 is deliberately unsupported.
// Adapted policies from sr-core sr_core/vpkg; see README.md and ATTRIBUTION.md.
// MIT License
//
// Copyright (c) 2026 Pedro Anisio Silva
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use crate::invalid;
use crate::{Error, Limits};
use filmcraft_bitstream::{crc32, flate};
use std::collections::BTreeSet;

/// One uncompressed archive entry, owned exactly once.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub bytes: Vec<u8>,
    pub executable: bool,
}
/// A borrowed input to the ZIP writer.
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub executable: bool,
}

fn bad(message: &str) -> Error {
    invalid("ZIP", "archive", message)
}
fn add(a: usize, b: usize) -> Result<usize, Error> {
    a.checked_add(b).ok_or_else(|| bad("offset overflow"))
}
fn part(b: &[u8], at: usize, len: usize) -> Result<&[u8], Error> {
    b.get(at..add(at, len)?).ok_or_else(|| bad("truncated record"))
}
fn word(b: &[u8], at: usize) -> Result<u16, Error> {
    let [a, b] = part(b, at, 2)? else {
        return Err(bad("truncated word"));
    };
    Ok(u16::from_le_bytes([*a, *b]))
}
fn dword(b: &[u8], at: usize) -> Result<u32, Error> {
    let [a, b, c, d] = part(b, at, 4)? else {
        return Err(bad("truncated dword"));
    };
    Ok(u32::from_le_bytes([*a, *b, *c, *d]))
}
fn size(b: &[u8], at: usize) -> Result<usize, Error> {
    let n = dword(b, at)?;
    if n == u32::MAX {
        return Err(bad("ZIP64 is not supported"));
    }
    usize::try_from(n).map_err(|_| bad("size exceeds address space"))
}
fn extra(b: &[u8]) -> Result<(), Error> {
    let mut at = 0;
    while at < b.len() {
        if word(b, at)? == 1 {
            return Err(bad("ZIP64 extra field is not supported"));
        }
        let len = usize::from(word(b, add(at, 2)?)?);
        at = add(at, 4)?;
        part(b, at, len)?;
        at = add(at, len)?;
    }
    Ok(())
}

/// Whether a file path is canonical and safe relative to a package root.
/// Colons and dot segments are also refused to avoid aliases on Windows.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && !path.chars().any(char::is_control)
        && path.split('/').all(|s| !s.is_empty() && s != "." && s != ".." && !s.ends_with(['.', ' ']))
}

pub(crate) fn unique_paths<'a>(paths: impl IntoIterator<Item = &'a str>, limits: &Limits) -> Result<(), Error> {
    let mut names = BTreeSet::new();
    let mut memory = 0usize;
    for path in paths {
        memory = memory.saturating_add(path.len().saturating_mul(8)).saturating_add(256);
        if memory > limits.max_memory_bytes {
            return Err(invalid("LIMIT", path, "path table exceeds memory budget"));
        }
        if names.len() >= limits.max_entries || path.len() > limits.max_name_bytes || !safe_path(path) {
            return Err(invalid("PATH", path, "unsafe path or path budget exceeded"));
        }
        // Lowercase first joins ß/ẞ; uppercase also folds ß/SS and final sigma/Σ.
        if !names.insert(path.to_lowercase().to_uppercase()) {
            return Err(invalid("PATH", path, "duplicate or case-colliding path"));
        }
    }
    for name in &names {
        for (i, _) in name.match_indices('/') {
            if name.get(..i).is_some_and(|p| names.contains(p)) {
                return Err(invalid("PATH", name, "file is also a parent directory"));
            }
        }
    }
    Ok(())
}

/// Read every ZIP entry in physical order, cross-checking the central directory.
/// Split archives, ZIP64, directory records, prefixes and unclaimed gaps are unsupported.
pub fn read(bytes: &[u8], limits: &Limits) -> Result<Vec<Entry>, Error> {
    if bytes.len() > limits.max_memory_bytes {
        return Err(bad("archive exceeds memory budget"));
    }
    let end = bytes.len().checked_sub(22).ok_or_else(|| bad("no EOCD"))?;
    let first = bytes.len().saturating_sub(65_557);
    let eocd = (first..=end)
        .rev()
        .find(|&i| {
            bytes.get(i..i.saturating_add(4)) == Some(b"PK\x05\x06")
                && word(bytes, i.saturating_add(20)).is_ok_and(|n| i.saturating_add(22).saturating_add(usize::from(n)) == bytes.len())
        })
        .ok_or_else(|| bad("no EOCD with a complete comment"))?;
    let tail = part(bytes, eocd, 22)?;
    if word(tail, 4)? != 0 || word(tail, 6)? != 0 || word(tail, 8)? != word(tail, 10)? {
        return Err(bad("multi-disk ZIP is not supported"));
    }
    let count = word(tail, 10)?;
    if count == u16::MAX {
        return Err(bad("ZIP64 entry count is not supported"));
    }
    let count = usize::from(count);
    if count > limits.max_entries {
        return Err(bad("entry count budget exceeded"));
    }
    let cd_size = size(tail, 12)?;
    let cd_start = size(tail, 16)?;
    if add(cd_start, cd_size)? != eocd {
        return Err(bad("data after central directory or invalid central directory size"));
    }
    let directory = part(bytes, cd_start, cd_size)?;
    let overhead = count.checked_mul(512).and_then(|n| n.checked_add(bytes.len())).ok_or_else(|| bad("memory size overflow"))?;
    if overhead > limits.max_memory_bytes {
        return Err(bad("entry metadata exceeds memory budget"));
    }
    let mut entries = Vec::new();
    entries.try_reserve_exact(count).map_err(|_| bad("cannot allocate entry table"))?;
    let mut cursor = 0;
    let mut local_end = 0;
    let mut expanded = 0usize;
    let mut memory = overhead;
    for _ in 0..count {
        let head = part(directory, cursor, 46)?;
        if dword(head, 0)? != 0x02014b50 {
            return Err(bad("invalid central directory signature"));
        }
        let flags = word(head, 8)?;
        let method = word(head, 10)?;
        if flags & !0x080e != 0 || !matches!(method, 0 | 8) {
            return Err(bad("encrypted entry, unsupported flags or compression method"));
        }
        if method == 0 && flags & 6 != 0 {
            return Err(bad("deflate flags on stored entry"));
        }
        let crc = dword(head, 16)?;
        let compressed = size(head, 20)?;
        let uncompressed = size(head, 24)?;
        let name_len = usize::from(word(head, 28)?);
        let extra_len = usize::from(word(head, 30)?);
        let comment_len = usize::from(word(head, 32)?);
        if word(head, 34)? != 0 {
            return Err(bad("multi-disk entry"));
        }
        let mode = dword(head, 38)? >> 16;
        if mode & 0o170000 == 0o120000 {
            return Err(bad("symbolic link entry"));
        }
        let offset = size(head, 42)?;
        if offset != local_end {
            return Err(bad("overlapping, reordered or unclaimed local entry data"));
        }
        if name_len > limits.max_name_bytes {
            return Err(bad("entry name budget exceeded"));
        }
        cursor = add(cursor, 46)?;
        let name_bytes = part(directory, cursor, name_len)?;
        if flags & 0x800 == 0 && !name_bytes.is_ascii() {
            return Err(bad("non-ASCII entry name requires the UTF-8 flag"));
        }
        let name = std::str::from_utf8(name_bytes).map_err(|_| bad("entry name is not UTF-8"))?;
        if !safe_path(name) {
            return Err(invalid("PATH", name, "unsafe ZIP name"));
        }
        cursor = add(cursor, name_len)?;
        extra(part(directory, cursor, extra_len)?)?;
        cursor = add(cursor, extra_len)?;
        part(directory, cursor, comment_len)?;
        cursor = add(cursor, comment_len)?;
        let local = part(bytes, offset, 30)?;
        if dword(local, 0)? != 0x04034b50 || word(local, 6)? != flags || word(local, 8)? != method {
            return Err(bad("local header signature, flags or method mismatch"));
        }
        let local_crc = dword(local, 14)?;
        let local_compressed = size(local, 18)?;
        let local_uncompressed = size(local, 22)?;
        if flags & 8 == 0 {
            if (local_crc, local_compressed, local_uncompressed) != (crc, compressed, uncompressed) {
                return Err(bad("local header sizes or CRC mismatch"));
            }
        } else if (local_crc != 0 && local_crc != crc)
            || (local_compressed != 0 && local_compressed != compressed)
            || (local_uncompressed != 0 && local_uncompressed != uncompressed)
        {
            return Err(bad("descriptor entry has conflicting local sizes or CRC"));
        }
        let local_name_len = usize::from(word(local, 26)?);
        let local_extra_len = usize::from(word(local, 28)?);
        let mut start = add(offset, 30)?;
        if part(bytes, start, local_name_len)? != name.as_bytes() {
            return Err(bad("local name mismatch"));
        }
        start = add(start, local_name_len)?;
        extra(part(bytes, start, local_extra_len)?)?;
        start = add(start, local_extra_len)?;
        let payload_end = add(start, compressed)?;
        if payload_end > cd_start {
            return Err(bad("entry extends into central directory"));
        }
        let payload = part(bytes, start, compressed)?;
        local_end = payload_end;
        if flags & 8 != 0 {
            let descriptor = part(bytes, payload_end, cd_start.saturating_sub(payload_end))?;
            // CRC can itself equal the optional signature; try the signature-free form first.
            let plain = descriptor_matches(descriptor, 0, crc, compressed, uncompressed);
            let signed = dword(descriptor, 0)? == 0x08074b50 && descriptor_matches(descriptor, 4, crc, compressed, uncompressed);
            local_end = add(
                local_end,
                if plain {
                    12
                } else if signed {
                    16
                } else {
                    return Err(bad("invalid data descriptor"));
                },
            )?;
        }
        memory = add(memory, name_len.checked_mul(8).ok_or_else(|| bad("name memory overflow"))?)?;
        let cap = limits.max_entry_bytes.min(limits.max_total_bytes.saturating_sub(expanded)).min(limits.max_memory_bytes.saturating_sub(memory) / 2);
        if uncompressed > cap {
            return Err(bad("entry expansion budget exceeded"));
        }
        let data = if method == 0 {
            if payload.len() > cap {
                return Err(bad("stored entry exceeds budget"));
            }
            let mut out = Vec::new();
            out.try_reserve_exact(payload.len()).map_err(|_| bad("cannot allocate stored entry"))?;
            out.extend_from_slice(payload);
            out
        } else {
            let (out, used) = flate::inflate_with_consumed(payload, cap).map_err(|e| bad(&e.to_string()))?;
            if used != compressed {
                return Err(bad("trailing compressed bytes"));
            }
            out
        };
        if data.len() != uncompressed || crc32(&data) != crc {
            return Err(bad("expanded size or CRC-32 mismatch"));
        }
        expanded = add(expanded, data.len())?;
        memory = add(memory, data.capacity())?;
        if memory > limits.max_memory_bytes {
            return Err(bad("allocated output exceeds memory budget"));
        }
        entries.push(Entry { path: name.to_owned(), bytes: data, executable: mode & 0o111 != 0 });
    }
    if cursor != directory.len() || local_end != cd_start {
        return Err(bad("unclaimed archive data"));
    }
    unique_paths(entries.iter().map(|e| e.path.as_str()), limits)?;
    Ok(entries)
}
fn descriptor_matches(b: &[u8], at: usize, crc: u32, compressed: usize, uncompressed: usize) -> bool {
    dword(b, at).is_ok_and(|v| v == crc)
        && size(b, at.saturating_add(4)).is_ok_and(|v| v == compressed)
        && size(b, at.saturating_add(8)).is_ok_and(|v| v == uncompressed)
}

struct Buffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl Buffer {
    fn put(&mut self, b: &[u8]) -> Result<(), Error> {
        if add(self.bytes.len(), b.len())? > self.limit {
            return Err(bad("writer memory budget exceeded"));
        }
        self.bytes.try_reserve_exact(b.len()).map_err(|_| bad("cannot allocate ZIP output"))?;
        self.bytes.extend_from_slice(b);
        Ok(())
    }
    fn word(&mut self, n: u16) -> Result<(), Error> {
        self.put(&n.to_le_bytes())
    }
    fn dword(&mut self, n: u32) -> Result<(), Error> {
        self.put(&n.to_le_bytes())
    }
    fn size(&mut self, n: usize) -> Result<(), Error> {
        let n = u32::try_from(n).ok().filter(|&n| n != u32::MAX).ok_or_else(|| bad("ZIP64 output is not supported"))?;
        self.dword(n)
    }
}
fn date_time(epoch: Option<u64>) -> Result<(u16, u16), Error> {
    let epoch = epoch.unwrap_or(315_532_800).max(315_532_800);
    if epoch >= 4_354_819_200 {
        return Err(bad("timestamp is past ZIP's 2107 limit"));
    }
    let mut days = epoch / 86_400;
    let seconds = epoch % 86_400;
    let mut year = 1970u16;
    while year < 2108 {
        let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let count = if leap { 366 } else { 365 };
        if days < count {
            break;
        }
        days -= count;
        year += 1;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let mut month = 1u16;
    for count in [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31] {
        if days < count {
            break;
        }
        days -= count;
        month += 1;
    }
    let day = u16::try_from(days + 1).map_err(|_| bad("invalid day"))?;
    let time = u16::try_from((seconds / 3600) << 11 | (seconds % 3600 / 60) << 5 | (seconds % 60 / 2)).map_err(|_| bad("invalid time"))?;
    Ok((time, (year - 1980) << 9 | month << 5 | day))
}
fn stored(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "gif"
            | "mp4"
            | "mov"
            | "mkv"
            | "webm"
            | "mp3"
            | "aac"
            | "m4a"
            | "ogg"
            | "opus"
            | "flac"
            | "zip"
            | "gz"
            | "xz"
            | "bz2"
            | "7z"
            | "glb"
            | "woff"
            | "woff2"
            | "exr"
            | "onnx"
    )
}

/// Write entries in caller order, with fixed DOS times and regular-file modes.
/// `epoch` is an explicit SOURCE_DATE_EPOCH; no environment or clock is consulted.
pub fn write(entries: &[Input<'_>], limits: &Limits, epoch: Option<u64>) -> Result<Vec<u8>, Error> {
    if entries.len() >= usize::from(u16::MAX) {
        return Err(bad("ZIP64 entry count is not supported"));
    }
    unique_paths(entries.iter().map(|e| e.path), limits)?;
    let (time, date) = date_time(epoch)?;
    let mut total = 0usize;
    let mut names = 0usize;
    for entry in entries {
        if entry.bytes.len() > limits.max_entry_bytes || entry.path.len() > usize::from(u16::MAX) {
            return Err(bad("writer entry budget exceeded"));
        }
        total = add(total, entry.bytes.len())?;
        names = add(names, entry.path.len())?;
    }
    if total > limits.max_total_bytes {
        return Err(bad("writer total expansion budget exceeded"));
    }
    // Payload copies, DEFLATE window/tokens, names and both ZIP tables must coexist.
    let overhead = add(total, add(names.saturating_mul(8), entries.len().saturating_mul(512))?)?;
    let scratch = entries.iter().filter(|e| !stored(e.path)).map(|e| e.bytes.len().saturating_mul(4).saturating_add(2 << 20)).max().unwrap_or(0);
    let available = limits.max_memory_bytes.checked_sub(add(overhead, scratch)?).ok_or_else(|| bad("writer working-memory budget exceeded"))?;
    let directory_bound = add(entries.len().saturating_mul(46), names)?;
    let mut output_bound = add(directory_bound, 22)?;
    for entry in entries {
        let payload_bound = if stored(entry.path) { entry.bytes.len() } else { add(entry.bytes.len().saturating_mul(2), 64)? };
        output_bound = add(output_bound, add(add(30, entry.path.len())?, payload_bound)?)?;
    }
    if add(output_bound, directory_bound)? > available {
        return Err(bad("ZIP output reservation exceeds memory budget"));
    }
    // Reserve both buffers once so growth never temporarily doubles a large archive.
    let mut out = Buffer { bytes: Vec::new(), limit: output_bound };
    let mut cd = Buffer { bytes: Vec::new(), limit: directory_bound };
    out.bytes.try_reserve_exact(output_bound).map_err(|_| bad("cannot reserve ZIP output"))?;
    cd.bytes.try_reserve_exact(directory_bound).map_err(|_| bad("cannot reserve ZIP directory"))?;
    if add(out.bytes.capacity(), cd.bytes.capacity())? > available {
        return Err(bad("ZIP reservations exceed memory budget"));
    }
    for e in entries {
        let compressed = if stored(e.path) { std::borrow::Cow::Borrowed(e.bytes) } else { std::borrow::Cow::Owned(flate::deflate(e.bytes)) };
        let method = if stored(e.path) { 0 } else { 8 };
        let crc = crc32(e.bytes);
        let offset = out.bytes.len();
        let name_len = u16::try_from(e.path.len()).map_err(|_| bad("name too long"))?;
        out.dword(0x04034b50)?;
        out.word(20)?;
        out.word(0x800)?;
        out.word(method)?;
        out.word(time)?;
        out.word(date)?;
        out.dword(crc)?;
        out.size(compressed.len())?;
        out.size(e.bytes.len())?;
        out.word(name_len)?;
        out.word(0)?;
        out.put(e.path.as_bytes())?;
        out.put(&compressed)?;
        cd.dword(0x02014b50)?;
        cd.word(0x0314)?;
        cd.word(20)?;
        cd.word(0x800)?;
        cd.word(method)?;
        cd.word(time)?;
        cd.word(date)?;
        cd.dword(crc)?;
        cd.size(compressed.len())?;
        cd.size(e.bytes.len())?;
        cd.word(name_len)?;
        for _ in 0..4 {
            cd.word(0)?;
        }
        cd.dword(if e.executable { 0o100755 << 16 } else { 0o100644 << 16 })?;
        cd.size(offset)?;
        cd.put(e.path.as_bytes())?;
        if add(out.bytes.capacity(), cd.bytes.capacity())? > available {
            return Err(bad("ZIP buffers exceed memory budget"));
        }
    }
    let offset = out.bytes.len();
    let cd_size = cd.bytes.len();
    if add(add(offset, cd_size)?, add(cd.bytes.capacity(), 22)?)? > available {
        return Err(bad("ZIP directory assembly exceeds memory budget"));
    }
    out.put(&cd.bytes)?;
    out.dword(0x06054b50)?;
    out.word(0)?;
    out.word(0)?;
    let count = u16::try_from(entries.len()).map_err(|_| bad("too many entries"))?;
    out.word(count)?;
    out.word(count)?;
    out.size(cd_size)?;
    out.size(offset)?;
    out.word(0)?;
    Ok(out.bytes)
}
