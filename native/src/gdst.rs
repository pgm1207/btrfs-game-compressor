//! Godot 3 GDST encoded textures. Unknown layouts stay unchanged.
//! Logical dimensions, flags, codec and alpha are preserved. Physical pixels
//! may be resized/quantized for explicit quality profiles, only when smaller.
use image::{codecs::{png::PngEncoder, webp::WebPEncoder}, ColorType, ImageEncoder, ImageFormat, RgbaImage};
use std::io::{self, Cursor};

fn bad(s: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, s) }
fn u16le(b: &[u8], p: usize) -> u16 { u16::from_le_bytes(b[p..p+2].try_into().unwrap()) }
fn u32le(b: &[u8], p: usize) -> u32 { u32::from_le_bytes(b[p..p+4].try_into().unwrap()) }

fn decode(bytes: &[u8]) -> io::Result<Option<RgbaImage>> {
    if bytes.len() < 28 || &bytes[..4] != b"GDST" { return Ok(None); }
    let (w, h) = (u16le(bytes, 4) as u32, u16le(bytes, 8) as u32);
    if w == 0 || h == 0 || w as u64 * h as u64 > 32_000_000 { return Ok(None); }
    let df = u32le(bytes, 16);
    // Single-level, encoded PNG/WebP only. Do not guess raw/VRAM/mipmap layouts.
    if df & !0x07ff_ffff != 0 || df & (1 << 23) != 0 || u32le(bytes, 20) != 1 { return Ok(None); }
    let png = df & (1 << 20) != 0;
    let webp = df & (1 << 21) != 0;
    if png == webp { return Ok(None); }
    if u32le(bytes, 24) as usize != bytes.len() - 28 { return Ok(None); }
    let payload = &bytes[28..];
    let (payload, format) = if webp {
        if !payload.starts_with(b"WEBP") { return Ok(None); }
        (&payload[4..], ImageFormat::WebP)
    } else {
        // Godot's PNG packer uses an ordinary PNG payload.
        (payload, ImageFormat::Png)
    };
    let dims = image::ImageReader::with_format(Cursor::new(payload), format)
        .into_dimensions().map_err(|e| bad(&format!("GDST dimensions: {e}")))?;
    if dims != (w, h) { return Ok(None); }
    image::load_from_memory_with_format(payload, format)
        .map(|image| Some(image.into_rgba8())).map_err(|e| bad(&format!("GDST image: {e}")))
}

pub fn transform_safe(bytes: &[u8], profile: &str) -> io::Result<Option<Vec<u8>>> {
    let mut target = match crate::gst2::target_for(profile)? { Some(t) => t, None => return Ok(None) };
    if bytes.len() < 28 || &bytes[..4] != b"GDST" { return Ok(None); }
    let (w, h) = (u16le(bytes, 4) as u32, u16le(bytes, 8) as u32);
    if crate::texture_policy::small_or_thin(w, h) { return Ok(None); }
    let original_w = (u16le(bytes, 6) as u32).max(w);
    let original_h = (u16le(bytes, 10) as u32).max(h);
    target.max_edge = crate::texture_policy::effective_edge(target.max_edge, original_w, original_h);
    target.color_bits = crate::texture_policy::color_bits(target.color_bits);
    let edge = w.max(h);
    let (nw, nh) = if edge > target.max_edge {
        ((w as u64 * target.max_edge as u64 / edge as u64).max(1) as u32,
         (h as u64 * target.max_edge as u64 / edge as u64).max(1) as u32)
    } else { (w, h) };
    if crate::texture_policy::output_too_thin(nw, nh) { return Ok(None); }
    transform_target(bytes, &target)
}

// Codec core; production callers use transform_safe to enforce quality guards.
#[cfg(test)]
fn transform(bytes: &[u8], profile: &str) -> io::Result<Option<Vec<u8>>> {
    let target = match crate::gst2::target_for(profile)? { Some(t) => t, None => return Ok(None) };
    transform_target(bytes, &target)
}

fn transform_target(bytes: &[u8], target: &crate::gst2::TextureTarget) -> io::Result<Option<Vec<u8>>> {
    let bits = target.color_bits;
    let original = match decode(bytes)? { Some(image) => image, None => return Ok(None) };
    let (w, h) = original.dimensions();
    let edge = w.max(h);
    let resized = edge > target.max_edge;
    if !resized && bits == 8 { return Ok(None); }
    let (nw, nh) = if resized {
        ((w as u64 * target.max_edge as u64 / edge as u64).max(1) as u32,
         (h as u64 * target.max_edge as u64 / edge as u64).max(1) as u32)
    } else { (w, h) };
    let mut image = if resized {
        image::imageops::resize(&original, nw, nh, image::imageops::FilterType::Lanczos3)
    } else { original };
    if bits < 8 {
        let levels = (1u16 << bits) - 1;
        for pixel in image.pixels_mut() {
            for c in &mut pixel.0[..3] {
                let q = (*c as u16 * levels + 127) / 255;
                *c = ((q * 255 + levels / 2) / levels) as u8;
            }
        }
    }
    let mut payload = Vec::new();
    if u32le(bytes, 16) & (1 << 21) != 0 {
        payload.extend_from_slice(b"WEBP");
        WebPEncoder::new_lossless(&mut payload).write_image(image.as_raw(), nw, nh, ColorType::Rgba8.into())
            .map_err(|e| bad(&format!("GDST encode: {e}")))?;
    } else {
        PngEncoder::new(&mut payload).write_image(image.as_raw(), nw, nh, ColorType::Rgba8.into())
            .map_err(|e| bad(&format!("GDST encode: {e}")))?;
    }
    if payload.len() + 28 >= bytes.len() { return Ok(None); }
    let mut out = bytes[..28].to_vec();
    if resized {
        // Godot exposes custom dimensions to scenes/atlases while uploading
        // the smaller image. Preserve existing overrides or original size.
        let logical_w = if u16le(bytes, 6) == 0 { w as u16 } else { u16le(bytes, 6) };
        let logical_h = if u16le(bytes, 10) == 0 { h as u16 } else { u16le(bytes, 10) };
        out[4..6].copy_from_slice(&(nw as u16).to_le_bytes());
        out[6..8].copy_from_slice(&logical_w.to_le_bytes());
        out[8..10].copy_from_slice(&(nh as u16).to_le_bytes());
        out[10..12].copy_from_slice(&logical_h.to_le_bytes());
    }
    out[24..28].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    let check = decode(&out)?.ok_or_else(|| bad("GDST output verification failed"))?;
    if check != image { return Err(bad("GDST encoded pixels differ")); }
    Ok(Some(out))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn fixture(w: u16, h: u16) -> Vec<u8> {
        let mut image = RgbaImage::new(w as u32, h as u32);
        let mut state = 0x12345678u32;
        for p in image.pixels_mut() {
            for c in &mut p.0 {
                state ^= state << 13; state ^= state >> 17; state ^= state << 5;
                *c = state as u8;
            }
        }
        let mut payload = b"WEBP".to_vec();
        WebPEncoder::new_lossless(&mut payload).write_image(image.as_raw(), w as u32, h as u32, ColorType::Rgba8.into()).unwrap();
        let mut out = b"GDST".to_vec();
        for dim in [w, 0, h, 0] { out.extend_from_slice(&dim.to_le_bytes()); }
        for value in [4u32, 0x07200000, 1, payload.len() as u32] { out.extend_from_slice(&value.to_le_bytes()); }
        out.extend(payload);
        out
    }
    #[test]
    fn downsizing_preserves_logical_dimensions_and_flags() {
        let input = fixture(1000, 600);
        let output = transform(&input, "ultra-performance").unwrap().unwrap();
        assert_eq!((u16le(&output, 4), u16le(&output, 8)), (640, 384));
        assert_eq!((u16le(&output, 6), u16le(&output, 10)), (1000, 600));
        assert_eq!(&input[12..24], &output[12..24]);
        assert!(output.len() < input.len());
        assert!(transform(&input, "native").unwrap().is_none());
        assert!(transform(&input, "lossless").unwrap().is_none());
    }
    #[test]
    fn quantization_preserves_alpha_and_unknown_layouts_are_skipped() {
        let input = fixture(256, 256);
        let output = transform(&input, "ultra-performance").unwrap().unwrap();
        let a = decode(&input).unwrap().unwrap();
        let b = decode(&output).unwrap().unwrap();
        assert!(a.pixels().zip(b.pixels()).all(|(a, b)| a[3] == b[3]));
        let mut unknown = input.clone(); unknown[20..24].copy_from_slice(&2u32.to_le_bytes());
        assert!(transform(&unknown, "ultra-performance").unwrap().is_none());
        assert!(transform(b"GDST", "ultra-performance").unwrap().is_none());
    }

    #[test]
    fn safe_profiles_preserve_small_and_thin_textures() {
        let small = fixture(512, 512);
        let thin = fixture(2048, 65);
        for profile in ["ultra-performance", "performance", "balanced", "quality", "ultra-quality", "native", "lossless"] {
            assert!(transform_safe(&small, profile).unwrap().is_none(), "{profile}");
            assert!(transform_safe(&thin, profile).unwrap().is_none(), "{profile}");
        }
        assert!(transform_safe(&fixture(1000, 600), "ultra-performance").unwrap().is_some());
    }

    #[test]
    fn relative_budget_is_retained_in_logical_overrides() {
        let source = fixture(2048, 1024);
        let out = transform_safe(&source, "ultra-performance").unwrap().unwrap();
        assert_eq!((u16le(&out, 4), u16le(&out, 8)), (1024, 512));
        assert_eq!((u16le(&out, 6), u16le(&out, 10)), (2048, 1024));
        assert!(transform_safe(&out, "ultra-performance").unwrap().is_none());
    }
}
