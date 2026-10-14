use super::*;
use proptest::prelude::*;

// RFC 1951 §3.2.4: BFINAL=1/BTYPE=00, padding, LEN=3, NLEN=!3, then abc.
const STORED: &[u8] = &[1, 3, 0, 0xfc, 0xff, b'a', b'b', b'c'];
// §3.2.6: header 110 (in transmission order), A=01110001, EOB=0000000, padding.
const FIXED: &[u8] = &[0x73, 0x04, 0];
// §3.2.7: HLIT=257, HDIST=1, HCLEN=18. CL18=1, CL0=CL1=2.
// CL18/127 + CL18/107 give 138+118 zero lengths; CL1 gives EOB length 1;
// CL0 gives no distance alphabet. The single EOB bit is zero. Output is empty.
const DYNAMIC: &[u8] = &[0x05, 0xc0, 0x81, 0x08, 0, 0, 0, 0, 0x20, 0x7f, 0xeb, 0x03];

#[test]
fn handmade_stored() {
    assert_eq!(inflate(STORED, 3).unwrap(), b"abc");
}
#[test]
fn handmade_fixed() {
    assert_eq!(inflate(FIXED, 1).unwrap(), b"A");
}
#[test]
fn handmade_dynamic() {
    assert_eq!(inflate(DYNAMIC, 0).unwrap(), b"");
    assert_eq!(inflate_with_consumed(DYNAMIC, 0).unwrap().1, DYNAMIC.len());
}
#[test]
fn adler_vectors() {
    assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    assert_eq!(adler32(b""), 1);
}
#[test]
fn pinned_zlib_bytes() {
    // Fixed literal codes and EOB, followed by hand-summed Adler-32.
    for (input, expected) in [
        (&b""[..], &b"\x78\x9c\x03\x00\x00\x00\x00\x01"[..]),
        (&b"A"[..], &b"\x78\x9c\x73\x04\x00\x00\x42\x00\x42"[..]),
        (&b"abc"[..], &b"\x78\x9c\x4b\x4c\x4a\x06\x00\x02\x4d\x01\x27"[..]),
    ] {
        assert_eq!(zlib_encode(input), expected);
        assert_eq!(zlib_decode(expected, input.len()).unwrap(), input);
        assert_eq!(deflate(input), expected[2..expected.len() - 4]);
    }
}
#[test]
fn consumed_bytes_exclude_read_ahead() {
    for (raw, expected) in [(STORED, &b"abc"[..]), (FIXED, &b"A"[..]), (DYNAMIC, &b""[..])] {
        let mut padded = raw.to_vec();
        padded.extend_from_slice(&[0x5a; 32]);
        assert_eq!(inflate_with_consumed(&padded, expected.len()).unwrap(), (expected.to_vec(), raw.len()));
        assert!(inflate(&padded, expected.len()).is_err());
        assert!(inflate_exact(&padded, expected.len()).is_err());
    }
}
#[test]
fn exact_size() {
    assert_eq!(inflate_exact(STORED, 3).unwrap(), b"abc");
    assert!(inflate_exact(STORED, 4).is_err());
}
#[test]
fn truncated_streams() {
    for raw in [STORED, FIXED, DYNAMIC] {
        for n in 0..raw.len() {
            assert!(inflate(&raw[..n], 100).is_err(), "{raw:?} truncated at {n}");
        }
    }
}
#[test]
fn bad_block_type() {
    assert!(inflate(&[7], 0).is_err());
}
#[test]
fn distance_before_output() {
    assert!(inflate(&[3, 2], 16).is_err());
}
#[test]
fn stored_length_mismatch() {
    assert!(inflate(&[1, 3, 0, 0, 0, 1, 2, 3], 3).is_err());
}
#[test]
fn output_limit() {
    assert!(inflate(STORED, 2).is_err());
    assert!(inflate(FIXED, 0).is_err());
    assert!(inflate(&deflate(&[42; 1000]), 999).is_err());
}
// Independent fixture writer: fields are transmitted LSB first; canonical codes are reversed manually.
fn fields(fields: &[(u32, u32)]) -> Vec<u8> {
    let mut out = vec![];
    let mut pos = 0;
    for &(v, n) in fields {
        for i in 0..n {
            if pos % 8 == 0 {
                out.push(0);
            }
            out[pos / 8] |= (((v >> i) & 1) as u8) << (pos % 8);
            pos += 1;
        }
    }
    out
}
#[test]
fn oversubscribed_huffman() {
    // Dynamic header then three one-bit CL symbols (16,17,18): Kraft sum 3/2.
    let raw = fields(&[(5, 3), (0, 5), (0, 5), (0, 4), (1, 3), (1, 3), (1, 3), (0, 3)]);
    assert!(inflate(&raw, 0).is_err());
}
#[test]
fn incomplete_huffman() {
    // Only CL0 has length 2: Kraft sum 1/4. Appended zeros must not excuse the tree.
    let mut raw = fields(&[(5, 3), (0, 5), (0, 5), (0, 4), (0, 3), (0, 3), (0, 3), (2, 3)]);
    raw.extend_from_slice(&[0; 100]);
    assert!(inflate(&raw, 0).is_err());
}
#[test]
fn zlib_hostile_header_trailer() {
    let valid = b"\x78\x9c\x73\x04\x00\x00\x42\x00\x42";
    assert_eq!(zlib_decode(valid, 1).unwrap(), b"A");
    for n in 0..valid.len() {
        assert!(zlib_decode(&valid[..n], 1).is_err());
    }
    for index in [0, 1, 5, 6, 7, 8] {
        let mut bad = valid.to_vec();
        bad[index] ^= 1;
        assert!(zlib_decode(&bad, 1).is_err());
    }
    assert!(zlib_decode(b"\x78\x20", 0).is_err()); // valid FCHECK with FDICT
    let mut trailing = valid.to_vec();
    trailing.push(0);
    assert!(zlib_decode(&trailing, 1).is_err());
    assert!(zlib_decode(valid, 2).is_err());
}
#[test]
fn roundtrip_boundaries() {
    let mut seed = 9u32;
    let noise: Vec<u8> = (0..65_537)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    for data in [vec![], vec![255], vec![42; 65_537], noise] {
        assert_eq!(inflate(&deflate(&data), data.len()).unwrap(), data);
        assert_eq!(zlib_decode(&zlib_encode(&data), data.len()).unwrap(), data);
    }
}
#[test]
fn mutations_never_panic() {
    for raw in [STORED, FIXED, DYNAMIC] {
        for i in 0..raw.len() {
            for bit in 0..8 {
                let mut bad = raw.to_vec();
                bad[i] ^= 1 << bit;
                assert!(std::panic::catch_unwind(|| inflate(&bad, 4096)).is_ok());
            }
        }
    }
    assert!(inflate(STORED, usize::MAX).is_ok()); // budget is a cap, not an allocation request
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn arbitrary_roundtrip(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        prop_assert_eq!(inflate(&deflate(&data), data.len()).unwrap(), data);
    }
    #[test]
    fn arbitrary_hostile(data in proptest::collection::vec(any::<u8>(), 0..256), limit in 0usize..4096) {
        prop_assert!(std::panic::catch_unwind(|| inflate(&data, limit)).is_ok());
    }
}

// A dynamic alphabet with only code-length symbols 0 and `width` (one bit each).
fn literal_tree(width: u32, distance_count: u32) -> Vec<u8> {
    let mut f = vec![(5, 3), (0, 5), (distance_count - 1, 5), (14, 4)];
    for symbol in [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1] {
        f.push((u32::from(symbol == 0 || symbol == width), 3));
    }
    for symbol in 0..257 {
        f.push((u32::from(symbol == 65 || symbol == 256), 1));
    }
    for _ in 0..distance_count {
        f.push((0, 1));
    }
    // For width 1: A=0, EOB=1. For width 2: A=00, EOB=01.
    f.push((0, width));
    f.push((1 << (width - 1), width));
    fields(&f)
}
#[test]
fn incomplete_literal_tree() {
    assert!(inflate(&literal_tree(2, 1), 1).is_err());
}
#[test]
fn unused_reserved_distance_lengths() {
    // HDIST permits 32 entries; the reserved last two are absent, as are all matches.
    assert_eq!(inflate(&literal_tree(1, 32), 1).unwrap(), b"A");
}

#[test]
fn legacy_encoder_fingerprints() {
    let mut seed = 9u32;
    let noise: Vec<u8> = (0..65_537)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    for (name, data) in [("repeated", vec![42; 65_537]), ("stored", noise.clone()), ("dynamic", noise.iter().map(|b| b % 4).collect())] {
        // Captured from the untouched export encoder before extraction (VM red run).
        let expected = match name {
            "repeated" => (86, 0x39f6_e4c1),
            "stored" => (65_553, 0x2979_c68c),
            _ => (19_749, 0xd35f_c6ad),
        };
        let encoded = zlib_encode(&data);
        assert_eq!((encoded.len(), crate::crc32(&encoded)), expected, "{name}");
    }
}
