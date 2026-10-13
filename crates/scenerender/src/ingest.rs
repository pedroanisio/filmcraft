//! Reading one rendered PNG frame back as a [`filmcraft_frame::VideoFrame`], with size and dimension checks.

use filmcraft_frame::VideoFrame;
use filmcraft_media::{MediaError, Result};
use std::path::Path;

/// Read a rendered PNG as straight-alpha sRGB RGBA8.
///
/// Rejects files above `max_bytes` or 256 MiB, and decoded RGBA8 frames above 256 MiB.
/// PNG dimensions are checked before decoding, including the allocation for 16-bit input.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_frame(path: &Path, width: u32, height: u32, max_bytes: u64) -> Result<VideoFrame> {
    use std::{fs::File, io::Read};

    const MAX_BYTES: u64 = 256 * 1024 * 1024;
    let io_error = |e| MediaError::Io(format!("{}: {e}", path.display()));
    let limit = max_bytes.min(MAX_BYTES);
    let metadata = std::fs::metadata(path).map_err(io_error)?;
    if !metadata.is_file() {
        return Err(MediaError::Io(format!("{}: expected a regular PNG file", path.display())));
    }
    if metadata.len() > limit {
        return Err(MediaError::Io(format!("{}: PNG file exceeds {limit}-byte limit", path.display())));
    }
    let mut file = File::open(path).map_err(io_error)?;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(e)),
        };
        if (bytes.len() as u64).saturating_add(count as u64) > limit {
            return Err(MediaError::Io(format!("{}: PNG file exceeds {limit}-byte limit", path.display())));
        }
        bytes.try_reserve_exact(count).map_err(|e| MediaError::Io(format!("{}: allocate PNG input: {e}", path.display())))?;
        let data = chunk.get(..count).ok_or_else(|| MediaError::Io("invalid PNG read byte count".into()))?;
        bytes.extend_from_slice(data);
    }
    let bad_header = || MediaError::Decode(format!("{}: invalid PNG header", path.display()));
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR") {
        return Err(bad_header());
    }
    let dimension = |range: std::ops::Range<usize>| -> Result<u32> {
        let value: [u8; 4] = bytes.get(range).ok_or_else(bad_header)?.try_into().map_err(|_| bad_header())?;
        Ok(u32::from_be_bytes(value))
    };
    let (png_width, png_height) = (dimension(16..20)?, dimension(20..24)?);
    let rgba_bytes = i128::from(width) * i128::from(height) * 4;
    if width == 0 || height == 0 || rgba_bytes > i128::from(MAX_BYTES) || (png_width, png_height) != (width, height) {
        return Err(MediaError::Decode(format!(
            "{}: PNG dimensions {png_width}×{png_height} do not match supported dimensions {width}×{height}",
            path.display()
        )));
    }
    let raster = filmcraft_media::still::decode_raster("scene-render.png", &bytes)?;
    if (raster.width, raster.height) != (width, height) || raster.rgba.len() as i128 != rgba_bytes {
        return Err(MediaError::Decode(format!("{}: decoded PNG dimensions or byte count do not match {width}×{height}", path.display())));
    }
    Ok(VideoFrame::rgba8(width, height, raster.rgba))
}

/// Rendered files are unavailable on wasm.
#[cfg(target_arch = "wasm32")]
pub fn read_frame(_path: &Path, _width: u32, _height: u32, _max_bytes: u64) -> Result<VideoFrame> {
    Err(MediaError::Unsupported("scene-render files are unavailable on wasm".into()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::fake::{Behaviour, FakeRenderer};
    use filmcraft_frame::PixelData;

    #[test]
    fn good_png_preserves_straight_alpha_and_srgb() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let path = fake.dir().join("good.png");
        image::RgbaImage::from_pixel(2, 3, image::Rgba([200, 100, 50, 128])).save(&path).unwrap();
        let frame = read_frame(&path, 2, 3, 4096).unwrap();
        assert_eq!((frame.width, frame.height), (2, 3));
        assert_eq!(frame.color, VideoFrame::rgba8(1, 1, vec![0; 4]).color);
        let PixelData::Rgba8(data) = frame.data else { panic!("not RGBA8") };
        assert_eq!(&data[..4], &[200, 100, 50, 128]);
    }

    #[test]
    fn wrong_size_is_rejected() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let path = fake.dir().join("wrong.png");
        image::RgbaImage::new(2, 3).save(&path).unwrap();
        assert!(matches!(read_frame(&path, 3, 2, 4096), Err(MediaError::Decode(s)) if s.contains("dimensions")));
    }

    #[test]
    fn corrupt_pngs_are_errors_not_panics() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let path = fake.dir().join("corrupt.png");
        image::RgbaImage::new(2, 3).save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        for len in [0, 4, 16, 32, original.len() / 2] {
            std::fs::write(&path, &original[..len]).unwrap();
            assert!(matches!(std::panic::catch_unwind(|| read_frame(&path, 2, 3, 4096)).unwrap(), Err(MediaError::Decode(_))));
        }
        for offset in 0..original.len() {
            for bit in 0..8 {
                let mut bytes = original.clone();
                bytes[offset] ^= 1 << bit;
                std::fs::write(&path, bytes).unwrap();
                let result = std::panic::catch_unwind(|| read_frame(&path, 2, 3, 4096)).unwrap();
                if let Ok(frame) = result {
                    assert_eq!((frame.width, frame.height, frame.byte_size()), (2, 3, 24));
                }
            }
        }
        std::fs::write(&path, b"not an image").unwrap();
        assert!(matches!(read_frame(&path, 2, 3, 4096), Err(MediaError::Decode(_))));
    }

    #[test]
    fn file_cap_is_checked_before_decode() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let path = fake.dir().join("oversize.png");
        std::fs::write(&path, b"garbage beyond the cap").unwrap();
        assert!(matches!(read_frame(&path, 1, 1, 4), Err(MediaError::Io(s)) if s.contains("limit")));
    }

    #[test]
    fn missing_file_is_io() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        assert!(matches!(read_frame(&fake.dir().join("missing.png"), 1, 1, 4096), Err(MediaError::Io(_))));
    }

    #[test]
    fn sixteen_bit_png_becomes_eight_bit() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let path = fake.dir().join("sixteen.png");
        image::ImageBuffer::from_pixel(1, 1, image::Rgba([65535_u16, 0, 32896, 32896])).save(&path).unwrap();
        let frame = read_frame(&path, 1, 1, 4096).unwrap();
        let PixelData::Rgba8(data) = frame.data else { panic!("not RGBA8") };
        assert_eq!(data.as_slice(), &[255, 0, 128, 128]);
    }

    #[test]
    fn hostile_dimensions_are_rejected_without_allocating() {
        let fake = FakeRenderer::new(Behaviour::NoOutput);
        let path = fake.dir().join("huge.png");
        image::RgbaImage::new(1, 1).save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        for (w, h) in [(0_u32, 1_u32), (u32::MAX, u32::MAX), (100_000, 100_000)] {
            let mut bytes = original.clone();
            bytes[16..20].copy_from_slice(&w.to_be_bytes());
            bytes[20..24].copy_from_slice(&h.to_be_bytes());
            std::fs::write(&path, bytes).unwrap();
            assert!(matches!(std::panic::catch_unwind(|| read_frame(&path, w, h, u64::MAX)).unwrap(), Err(MediaError::Decode(_))));
        }
    }
}
