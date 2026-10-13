//! The slate shown in place of rendered frames when scene-render is unavailable.
//! Original procedural artwork under MIT OR Apache-2.0.

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

use filmcraft_frame::VideoFrame;
use filmcraft_media::{MediaError, Result};

/// Build an opaque sRGB slate with diagonal stripes and an amber band.
/// Dimensions must be nonzero, at most 16384 per side, and fit the 256 MiB pixel budget.
pub fn slate(width: u32, height: u32) -> Result<VideoFrame> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(MediaError::Decode("scene slate dimensions must be between 1 and 16384".into()));
    }
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(4))
        .filter(|n| *n <= 256 * 1024 * 1024)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| MediaError::Decode("scene slate exceeds the 256 MiB pixel limit".into()))?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(bytes).map_err(|e| MediaError::Decode(format!("allocate scene slate: {e}")))?;
    let band_height = (height / 12).max(1);
    let band_start = (height - band_height) / 2;
    for y in 0..height {
        for x in 0..width {
            let color = if y >= band_start && y < band_start + band_height {
                [116, 89, 46, 255]
            } else if ((x + y) / 16) % 2 == 0 {
                [32, 32, 32, 255]
            } else {
                [46, 46, 46, 255]
            };
            pixels.extend_from_slice(&color);
        }
    }
    Ok(VideoFrame::rgba8(width, height, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_frame::PixelData;

    #[test]
    fn slate_size_and_determinism() {
        let a = slate(96, 64).unwrap();
        let b = slate(96, 64).unwrap();
        assert_eq!((a.width, a.height), (96, 64));
        assert_eq!(a.to_rgba8(), b.to_rgba8());
        assert_eq!(a.to_rgba8().len(), 96 * 64 * 4);
        assert!(matches!(a.data, PixelData::Rgba8(_)));
        assert_eq!(a.color, VideoFrame::rgba8(1, 1, vec![0; 4]).color);
    }

    #[test]
    fn slate_is_opaque_with_diagonal_greys_and_amber_band() {
        let bytes = slate(96, 64).unwrap().to_rgba8();
        assert!(bytes.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        let colors: std::collections::BTreeSet<_> = bytes.as_chunks::<4>().0.iter().copied().collect();
        assert_eq!(colors.len(), 3);
        assert_eq!(colors.iter().filter(|p| p[0] == p[1] && p[1] == p[2] && p[0] < 80).count(), 2);
        assert!(colors.iter().any(|p| p[0] > p[1] && p[1] > p[2]));
        let pixel = |x: usize, y: usize| &bytes[(y * 96 + x) * 4..(y * 96 + x + 1) * 4];
        assert_eq!(pixel(16, 0), pixel(15, 1));
        assert_ne!(pixel(0, 0), pixel(16, 0));
        assert!(pixel(0, 32)[0] > pixel(0, 32)[1]);
    }

    #[test]
    fn slate_refuses_zero_size() {
        for (w, h) in [(0, 0), (0, 10), (10, 0)] {
            assert!(slate(w, h).is_err(), "{w}x{h}");
        }
    }

    #[test]
    fn slate_refuses_overflow_and_oversize() {
        for (w, h) in [(u32::MAX, u32::MAX), (16385, 1), (1, 16385)] {
            assert!(slate(w, h).is_err(), "{w}x{h}");
        }
    }
}
