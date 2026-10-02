//! Profile-aware rewriting of Godot `GST2` (`.ctex`) textures, used by both the
//! export commands and the in-place asset pipeline.
//!
//! A GST2 file is a small outer header followed by an image payload that may be
//! raw pixels, PNG or WebP. Encodings this module does not understand are left
//! untouched by returning `Ok(None)`. Downscaling and re-encoding is a quality
//! tradeoff intended for the explicit lossy profiles; Native/Lossless never call
//! into here. The transform only returns bytes when they are strictly smaller
//! than the input, so it can never inflate a texture.
use image::{codecs::{png::PngEncoder, webp::WebPEncoder}, ColorType, ImageEncoder, RgbaImage};
use std::{io, path::Path};

pub const ENC_IMAGE: u32 = 0;
pub const ENC_PNG: u32 = 1;
pub const ENC_WEBP: u32 = 2;

pub const FMT_L8: u32 = 0;
pub const FMT_LA8: u32 = 1;
pub const FMT_RGB8: u32 = 4;
pub const FMT_RGBA8: u32 = 5;
pub const FMT_BC1: u32 = 17;
pub const FMT_BC2: u32 = 18;
pub const FMT_BC3: u32 = 19;
pub const FMT_BC7: u32 = 22;

#[derive(Clone, Copy)]
pub struct TextureTarget {
    pub max_edge: u32,
    pub quality: image_dds::Quality,
    pub color_bits: u8,
}
pub fn target_for(profile: &str) -> io::Result<Option<TextureTarget>> {
    let (max_edge, quality) = match profile {
        "ultra-performance" => (640, image_dds::Quality::Fast),
        "performance" => (1280, image_dds::Quality::Fast),
        "balanced" => (1920, image_dds::Quality::Normal),
        "quality" => (2560, image_dds::Quality::Normal),
        "ultra-quality" => (3840, image_dds::Quality::Slow),
        "lossless" => return Ok(None),
        "native" => return Ok(None),
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "texture profile must be a quality profile or native")),
    };
    let color_bits = match profile { "ultra-performance" => 4, "performance" => 6, "balanced" => 7, _ => 8 };
    Ok(Some(TextureTarget { max_edge, quality, color_bits }))
}

fn bad(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn le_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn div_ceil(v: u32, d: u32) -> u32 {
    v / d + u32::from(v % d != 0)
}
fn block_bytes(format: u32) -> Option<u32> {
    match format {
        FMT_BC1 => Some(8),
        FMT_BC2 | FMT_BC3 | FMT_BC7 => Some(16),
        _ => None,
    }
}
fn dds_format(format: u32) -> Option<image_dds::ImageFormat> {
    Some(match format {
        FMT_BC1 => image_dds::ImageFormat::BC1RgbaUnorm,
        FMT_BC2 => image_dds::ImageFormat::BC2RgbaUnorm,
        FMT_BC3 => image_dds::ImageFormat::BC3RgbaUnorm,
        FMT_BC7 => image_dds::ImageFormat::BC7RgbaUnorm,
        _ => return None,
    })
}

fn decode_bc(data: &[u8], width: u32, height: u32, format: u32) -> io::Result<RgbaImage> {
    let image_format = dds_format(format).ok_or_else(|| bad("unsupported BC format"))?;
    let expected = block_bytes(format).unwrap() as usize
        * div_ceil(width, 4) as usize
        * div_ceil(height, 4) as usize;
    if data.len() < expected {
        return Err(bad("truncated BC texture payload"));
    }
    let surface = image_dds::Surface {
        width,
        height,
        depth: 1,
        layers: 1,
        mipmaps: 1,
        image_format,
        data: &data[..expected],
    };
    let rgba = surface.decode_rgba8().map_err(|e| bad(&format!("BC decode failed: {e}")))?;
    rgba.to_image(0).map_err(|e| bad(&format!("BC image conversion failed: {e}")))
}
fn encode_bc(image: &RgbaImage, format: u32, quality: image_dds::Quality) -> io::Result<Vec<u8>> {
    let image_format = dds_format(format).ok_or_else(|| bad("unsupported BC format"))?;
    let surface = image_dds::SurfaceRgba8::from_image(image);
    let encoded = surface
        .encode(image_format, quality, image_dds::Mipmaps::Disabled)
        .map_err(|e| bad(&format!("BC encode failed: {e}")))?;
    Ok(encoded.data)
}
fn decode_rgba(data: &[u8], width: u32, height: u32, format: u32) -> io::Result<RgbaImage> {
    let pixels = (width as usize) * (height as usize);
    let mut out = Vec::with_capacity(pixels * 4);
    match format {
        FMT_RGBA8 => {
            if data.len() < pixels * 4 {
                return Err(bad("truncated RGBA8 payload"));
            }
            out.extend_from_slice(&data[..pixels * 4]);
        }
        FMT_RGB8 => {
            if data.len() < pixels * 3 {
                return Err(bad("truncated RGB8 payload"));
            }
            for px in data[..pixels * 3].chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        FMT_L8 => {
            if data.len() < pixels {
                return Err(bad("truncated L8 payload"));
            }
            for &v in &data[..pixels] {
                out.extend_from_slice(&[v, v, v, 255]);
            }
        }
        FMT_LA8 => {
            if data.len() < pixels * 2 {
                return Err(bad("truncated LA8 payload"));
            }
            for px in data[..pixels * 2].chunks_exact(2) {
                out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        _ => return Err(bad("unsupported raw image format")),
    }
    RgbaImage::from_raw(width, height, out).ok_or_else(|| bad("invalid raw image dimensions"))
}
fn encode_raw(image: &RgbaImage, format: u32) -> io::Result<Vec<u8>> {
    match format {
        FMT_RGBA8 => Ok(image.as_raw().clone()),
        FMT_RGB8 => Ok(image.pixels().flat_map(|p| [p.0[0], p.0[1], p.0[2]]).collect()),
        FMT_L8 => Ok(image.pixels().map(|p| p.0[0]).collect()),
        FMT_LA8 => Ok(image.pixels().flat_map(|p| [p.0[0], p.0[3]]).collect()),
        _ => Err(bad("unsupported raw image format")),
    }
}
fn encode_webp(image: &RgbaImage) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .write_image(image.as_raw(), image.width(), image.height(), ColorType::Rgba8.into())
        .map_err(|e| bad(&format!("WebP encode failed: {e}")))?;
    Ok(out)
}
fn encode_png(image: &RgbaImage) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(image.as_raw(), image.width(), image.height(), ColorType::Rgba8.into())
        .map_err(|e| bad(&format!("PNG encode failed: {e}")))?;
    Ok(out)
}

fn resize(image: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    image::imageops::resize(image, width, height, image::imageops::FilterType::Lanczos3)
}

/// Rewrite a `.ctex` GST2 texture down to the profile's longest edge. Returns
/// `Ok(None)` when the texture is unsupported, already small enough, or would not
/// become strictly smaller. Only the base mip chain is regenerated; header flags,
/// mipmap-limit and the declared pixel format are preserved.
pub fn transform_safe(bytes: &[u8], target: &TextureTarget) -> io::Result<Option<Vec<u8>>> {
    let Some(info) = inspect(bytes) else { return Ok(None); };
    if crate::texture_policy::small_or_thin(info.width, info.height) {
        return Ok(None);
    }
    // Without logical dimensions we cannot establish a stable original-size
    // budget. Leave such layouts untouched rather than repeatedly shrinking.
    let original_w = le_u32(bytes, 8);
    let original_h = le_u32(bytes, 12);
    if original_w == 0 || original_h == 0 { return Ok(None); }
    let target = TextureTarget {
        max_edge: crate::texture_policy::effective_edge(target.max_edge, original_w.max(info.width), original_h.max(info.height)),
        color_bits: crate::texture_policy::color_bits(target.color_bits),
        ..*target
    };
    let scale = (target.max_edge as f64 / info.width.max(info.height) as f64).min(1.0);
    let w = ((info.width as f64 * scale).round() as u32).max(1);
    let h = ((info.height as f64 * scale).round() as u32).max(1);
    if crate::texture_policy::output_too_thin(w, h) { return Ok(None); }
    transform(bytes, &target)
}

// Codec core; production callers use transform_safe to enforce quality guards.
fn transform(bytes: &[u8], target: &TextureTarget) -> io::Result<Option<Vec<u8>>> {
    if bytes.len() < 52 || &bytes[..4] != b"GST2" {
        return Ok(None);
    }
    let version = le_u32(bytes, 4);
    if version > 1 {
        return Ok(None);
    }
    let width = le_u16(bytes, 40) as u32;
    let height = le_u16(bytes, 42) as u32;
    let mipmaps = le_u32(bytes, 44);
    let format = le_u32(bytes, 48);
    let encoding = le_u32(bytes, 36);
    if width == 0 || height == 0 || mipmaps > 16 || width > 32768 || height > 32768 || width as u64 * height as u64 > 32_000_000 {
        return Ok(None);
    }
    if width.max(height) <= target.max_edge && target.color_bits == 8 {
        return Ok(None);
    }
    // Decode the base level only; the mip chain is regenerated from it.
    let base = match encoding {
        ENC_WEBP | ENC_PNG => {
            if bytes.len() < 56 {
                return Ok(None);
            }
            let len = le_u32(bytes, 52) as usize;
            if 56usize.checked_add(len).filter(|v| *v <= bytes.len()).is_none() {
                return Ok(None);
            }
            let payload = &bytes[56..56 + len];
            let decoded = match image::load_from_memory(payload) {
                Ok(v) => v.to_rgba8(),
                Err(_) => return Ok(None),
            };
            if decoded.width() != width || decoded.height() != height {
                // The declared size disagrees with the payload; leave it alone.
                return Ok(None);
            }
            decoded
        }
        ENC_IMAGE => {
            let decoded = match block_bytes(format) {
                Some(_) => match decode_bc(&bytes[52..], width, height, format) {
                    Ok(v) => v,
                    Err(_) => return Ok(None),
                },
                None => match decode_rgba(&bytes[52..], width, height, format) {
                    Ok(v) => v,
                    Err(_) => return Ok(None),
                },
            };
            decoded
        }
        _ => return Ok(None),
    };

    let scale = (target.max_edge as f64 / width.max(height) as f64).min(1.0);
    let new_w = ((width as f64 * scale).round() as u32).max(1);
    let new_h = ((height as f64 * scale).round() as u32).max(1);
    // Godot expects a complete mip chain for the new physical dimensions.
    let new_mipmaps = if mipmaps == 0 { 0 } else { 31 - new_w.max(new_h).leading_zeros() };
    let levels = new_mipmaps + 1;

    // Resampling to the same dimensions is not an identity operation (Lanczos3
    // ringing changes pixels slightly). Skip it when there is no downscale.
    let mut level = if new_w == width && new_h == height {
        base
    } else {
        resize(&base, new_w, new_h)
    };
    let mut payload = Vec::new();
    let mut cursor_w = new_w;
    let mut cursor_h = new_h;
    for i in 0..levels {
        if target.color_bits < 8 {
            let levels = (1u16 << target.color_bits) - 1;
            for pixel in level.pixels_mut() {
                for c in &mut pixel.0[..3] {
                    let q = (*c as u16 * levels + 127) / 255;
                    *c = ((q * 255 + levels / 2) / levels) as u8;
                }
            }
        }
        let encoded = match encoding {
            ENC_WEBP => encode_webp(&level)?,
            ENC_PNG => encode_png(&level)?,
            ENC_IMAGE => match block_bytes(format) {
                Some(_) => encode_bc(&level, format, target.quality)?,
                None => encode_raw(&level, format)?,
            },
            _ => return Ok(None),
        };
        if encoding == ENC_WEBP || encoding == ENC_PNG {
            payload.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
        }
        payload.extend_from_slice(&encoded);
        if i + 1 < levels {
            cursor_w = (cursor_w / 2).max(1);
            cursor_h = (cursor_h / 2).max(1);
            // Derive each mip from the already-quantized level above it, not the
            // pristine base. Otherwise a second in-place pass regenerates the
            // mips from the re-decoded base and shrinks slightly again forever.
            level = resize(&level, cursor_w, cursor_h);
        }
    }

    let mut out = Vec::with_capacity(52 + payload.len());
    out.extend_from_slice(b"GST2");
    out.extend_from_slice(&bytes[4..8]); // version
    // Outer dimensions are logical size overrides, not payload dimensions.
    // Changing them breaks scenes/atlases even when the pixels decode correctly.
    out.extend_from_slice(&bytes[8..16]);
    out.extend_from_slice(&bytes[16..36]); // data-format flags, mipmap limit, reserved
    out.extend_from_slice(&encoding.to_le_bytes());
    out.extend_from_slice(&(new_w as u16).to_le_bytes());
    out.extend_from_slice(&(new_h as u16).to_le_bytes());
    out.extend_from_slice(&new_mipmaps.to_le_bytes());
    out.extend_from_slice(&format.to_le_bytes());
    out.extend_from_slice(&payload);

    if out.len() >= bytes.len() {
        return Ok(None);
    }
    Ok(Some(out))
}

/// Read-only summary of a GST2 texture without decoding pixels.
#[allow(dead_code)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub encoding: u32,
    pub format: u32,
    pub mipmaps: u32,
}
#[allow(dead_code)]
pub fn inspect(bytes: &[u8]) -> Option<Info> {
    if bytes.len() < 52 || &bytes[..4] != b"GST2" || le_u32(bytes, 4) > 1 {
        return None;
    }
    let width = le_u16(bytes, 40) as u32;
    let height = le_u16(bytes, 42) as u32;
    let encoding = le_u32(bytes, 36);
    let mipmaps = le_u32(bytes, 44);
    let format = le_u32(bytes, 48);
    (width > 0 && height > 0 && encoding <= 3).then_some(Info { width, height, encoding, format, mipmaps })
}

pub fn is_ctex_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ctex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webp_ctex(width: u32, height: u32) -> Vec<u8> {
        let image = RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8, 255])
        });
        let payload = encode_webp(&image).unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(b"GST2");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // data-format flags
        out.extend_from_slice(&0u32.to_le_bytes()); // mipmap limit
        out.extend_from_slice(&[0u8; 12]); // reserved
        out.extend_from_slice(&ENC_WEBP.to_le_bytes());
        out.extend_from_slice(&(width as u16).to_le_bytes());
        out.extend_from_slice(&(height as u16).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // mipmaps
        out.extend_from_slice(&FMT_RGBA8.to_le_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }
    fn bc_ctex(format: u32, width: u32, height: u32) -> Vec<u8> {
        let image = RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x * 3 % 256) as u8, (y * 5 % 256) as u8, 128, 255])
        });
        let payload = encode_bc(&image, format, image_dds::Quality::Fast).unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(b"GST2");
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 12]);
        out.extend_from_slice(&ENC_IMAGE.to_le_bytes());
        out.extend_from_slice(&(width as u16).to_le_bytes());
        out.extend_from_slice(&(height as u16).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&format.to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    #[test]
    fn round_trips_bc_formats_through_decode_and_encode() {
        for format in [FMT_BC1, FMT_BC3, FMT_BC7] {
            let image = RgbaImage::from_fn(37, 19, |x, y| {
                image::Rgba([(x * 7 % 256) as u8, (y * 11 % 256) as u8, 64, 200])
            });
            let encoded = encode_bc(&image, format, image_dds::Quality::Fast).unwrap();
            let decoded = decode_bc(&encoded, 37, 19, format).unwrap();
            assert_eq!(decoded.dimensions(), (37, 19));
            // BC1 has no interpolated alpha; BC3/BC7 do. Colour must stay close.
            let a = image.get_pixel(10, 5).0;
            let b = decoded.get_pixel(10, 5).0;
            assert!((i32::from(a[0]) - i32::from(b[0])).abs() < 40, "{format}: {a:?} vs {b:?}");
        }
    }

    #[test]
    fn downscales_supported_textures_and_preserves_format() {
        let target = TextureTarget { max_edge: 32, quality: image_dds::Quality::Fast, color_bits: 8 };
        for source in [webp_ctex(128, 96), bc_ctex(FMT_BC7, 128, 96), bc_ctex(FMT_BC1, 128, 96)] {
            let out = transform(&source, &target).unwrap().expect("should shrink");
            assert!(out.len() < source.len());
            let before = inspect(&source).unwrap();
            let after = inspect(&out).unwrap();
            assert!(after.width.max(after.height) <= 32);
            assert_eq!(after.encoding, before.encoding);
            assert_eq!(after.format, before.format);
            assert_eq!(after.mipmaps, before.mipmaps);
            assert_eq!(&out[8..16], &source[8..16], "logical size overrides must stay intact");
            // The rewritten payload must still decode.
            let enc = le_u32(&out, 36);
            let fmt = le_u32(&out, 48);
            let len = le_u32(&out, 52) as usize;
            if enc == ENC_IMAGE {
                decode_bc(&out[52..], after.width, after.height, fmt).unwrap();
            } else {
                image::load_from_memory(&out[56..56 + len]).unwrap();
            }
        }
    }

    #[test]
    fn leaves_small_and_unsupported_textures_untouched() {
        let target = TextureTarget { max_edge: 32, quality: image_dds::Quality::Fast, color_bits: 8 };
        assert!(transform(&webp_ctex(16, 16), &target).unwrap().is_none());
        assert!(transform(&webp_ctex(128, 96), &target).unwrap().is_some());
        // Basis Universal / unknown encodings are skipped, never guessed.
        let mut basis = webp_ctex(128, 96);
        basis[36..40].copy_from_slice(&3u32.to_le_bytes());
        assert!(transform(&basis, &target).unwrap().is_none());
        assert!(transform(b"not a texture", &target).unwrap().is_none());
    }

    #[test]
    fn safe_profiles_preserve_small_and_thin_textures_without_quantizing() {
        let small = webp_ctex(512, 512);
        let thin = webp_ctex(2048, 65);
        for profile in ["ultra-performance", "performance", "balanced", "quality", "ultra-quality"] {
            let target = target_for(profile).unwrap().unwrap();
            assert!(transform_safe(&small, &target).unwrap().is_none(), "{profile}");
            assert!(transform_safe(&thin, &target).unwrap().is_none(), "{profile}");
        }
        let large = webp_ctex(1000, 600);
        assert!(transform_safe(&large, &target_for("ultra-performance").unwrap().unwrap()).unwrap().is_some());
        assert!(target_for("native").unwrap().is_none());
        assert!(target_for("lossless").unwrap().is_none());
    }

    #[test]
    fn relative_budget_uses_original_dimensions_and_cannot_shrink_repeatedly() {
        let source = webp_ctex(2048, 1024);
        let target = target_for("ultra-performance").unwrap().unwrap();
        let out = transform_safe(&source, &target).unwrap().unwrap();
        let info = inspect(&out).unwrap();
        assert_eq!((info.width, info.height), (1024, 512));
        assert_eq!(&out[8..16], &source[8..16]);
        assert!(transform_safe(&out, &target).unwrap().is_none());
        let mut unknown_size = source.clone();
        unknown_size[8..16].fill(0);
        assert!(transform_safe(&unknown_size, &target).unwrap().is_none());
    }

    #[test]
    fn at_cap_textures_are_quantized_once_and_then_untouched() {
        // Ultra Performance-style quantization at or below the physical cap must
        // be a fixed point: a later in-place pass must not resample or re-encode
        // the texture again. The mipped case is the one that regressed on real
        // Godot 4 packs, where regenerated mips differed on every run.
        let mut mipped = webp_ctex(256, 192);
        mipped[44..48].copy_from_slice(&3u32.to_le_bytes());
        for _ in 0..3 {
            mipped.extend_from_slice(&1u32.to_le_bytes());
            mipped.push(0);
        }
        let target = TextureTarget { max_edge: 64, quality: image_dds::Quality::Fast, color_bits: 4 };
        for source in [webp_ctex(48, 32), webp_ctex(256, 192), mipped] {
            let once = transform(&source, &target).unwrap().expect("first pass must reduce");
            assert!(once.len() < source.len());
            assert!(transform(&once, &target).unwrap().is_none(), "second pass must be a no-op");
            assert_eq!(once, transform(&source, &target).unwrap().unwrap(), "must be deterministic");
        }
    }

    #[test]
    fn downscaled_mipmaps_form_a_complete_chain_with_original_logical_size() {
        let mut source = webp_ctex(128, 96);
        source[44..48].copy_from_slice(&7u32.to_le_bytes());
        for i in 1..=7 {
            let image = RgbaImage::from_pixel((128 >> i).max(1), (96 >> i).max(1), image::Rgba([20, 40, 60, 255]));
            let payload = encode_webp(&image).unwrap();
            source.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            source.extend_from_slice(&payload);
        }
        let target = TextureTarget { max_edge: 32, quality: image_dds::Quality::Fast, color_bits: 4 };
        let out = transform(&source, &target).unwrap().unwrap();
        assert_eq!(&out[8..16], &source[8..16]);
        assert_eq!(le_u32(&out, 44), 5);
        let mut pos = 52;
        for i in 0..=5 {
            let len = le_u32(&out, pos) as usize; pos += 4;
            let image = image::load_from_memory(&out[pos..pos + len]).unwrap();
            assert_eq!((image.width(), image.height()), ((32 >> i).max(1), (24 >> i).max(1)));
            pos += len;
        }
        assert_eq!(pos, out.len());
    }
}
