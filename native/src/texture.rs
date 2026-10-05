//! Export-only DDS texture downscaling. Never writes in place and never enables
//! the main asset pipeline. Decodes a bounded 2D DDS, downscales the base mip to
//! a maximum dimension, re-encodes to the *same* codec with a regenerated mip
//! chain, and verifies the result by re-parsing it. The exported payload is
//! lossy and is not validated in-game.
//!
//! Supported inputs are 2D legacy BC1/BC2/BC3, legacy 32-bit BGRA/RGBA, and
//! DX10 BC1/BC2/BC3/BC4/BC5/BC7 plus R8G8B8A8/B8G8R8A8. Cubemaps, arrays,
//! volumes, BC6H and float formats are rejected rather than guessed.
use image::{imageops::FilterType, RgbaImage};
use image_dds::{ImageFormat, Mipmaps, Quality, Surface, SurfaceRgba8};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

const MAX_FILE: u64 = 512 * 1024 * 1024;
const MAX_PIXELS: u64 = 268_435_456; // 16384 x 16384

fn bad(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// `(block_width, block_height, bytes_per_block)` for the block-compressed
/// formats this module handles. `None` means the format is not block-compressed.
fn block_dims(format: ImageFormat) -> Option<(u32, u32, u64)> {
    use ImageFormat::*;
    Some(match format {
        BC1RgbaUnorm | BC1RgbaUnormSrgb => (4, 4, 8),
        BC2RgbaUnorm | BC2RgbaUnormSrgb => (4, 4, 16),
        BC3RgbaUnorm | BC3RgbaUnormSrgb => (4, 4, 16),
        BC4RUnorm | BC4RSnorm => (4, 4, 8),
        BC5RgUnorm | BC5RgSnorm => (4, 4, 16),
        BC7RgbaUnorm | BC7RgbaUnormSrgb => (4, 4, 16),
        _ => return None,
    })
}

/// Bytes per pixel for the uncompressed formats this module handles.
fn bytes_per_pixel(format: ImageFormat) -> Option<u64> {
    use ImageFormat::*;
    Some(match format {
        R8Unorm | R8Snorm => 1,
        Rg8Unorm | Rg8Snorm => 2,
        Rgba8Unorm | Rgba8UnormSrgb | Rgba8Snorm => 4,
        Bgra8Unorm | Bgra8UnormSrgb => 4,
        _ => return None,
    })
}

fn mip_size(format: ImageFormat, width: u32, height: u32) -> Option<u64> {
    if let Some((bw, bh, bytes)) = block_dims(format) {
        Some(width.div_ceil(bw) as u64 * height.div_ceil(bh) as u64 * bytes)
    } else {
        bytes_per_pixel(format).map(|bpp| width as u64 * height as u64 * bpp)
    }
}

fn chain_size(format: ImageFormat, width: u32, height: u32, mipmaps: u32) -> Option<u64> {
    let (mut w, mut h) = (width, height);
    let mut total = 0u64;
    for _ in 0..mipmaps {
        total = total.checked_add(mip_size(format, w, h)?)?;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    Some(total)
}

fn dxgi_format(value: u32) -> Option<ImageFormat> {
    use ImageFormat::*;
    Some(match value {
        28 => Rgba8Unorm,
        29 => Rgba8UnormSrgb,
        87 => Bgra8Unorm,
        91 => Bgra8UnormSrgb,
        71 => BC1RgbaUnorm,
        72 => BC1RgbaUnormSrgb,
        74 => BC2RgbaUnorm,
        75 => BC2RgbaUnormSrgb,
        77 => BC3RgbaUnorm,
        78 => BC3RgbaUnormSrgb,
        80 => BC4RUnorm,
        81 => BC4RSnorm,
        83 => BC5RgUnorm,
        84 => BC5RgSnorm,
        98 => BC7RgbaUnorm,
        99 => BC7RgbaUnormSrgb,
        _ => return None,
    })
}

fn dxgi_of(format: ImageFormat) -> Option<u32> {
    use ImageFormat::*;
    Some(match format {
        Rgba8Unorm => 28,
        Rgba8UnormSrgb => 29,
        Bgra8Unorm => 87,
        Bgra8UnormSrgb => 91,
        BC1RgbaUnorm => 71,
        BC1RgbaUnormSrgb => 72,
        BC2RgbaUnorm => 74,
        BC2RgbaUnormSrgb => 75,
        BC3RgbaUnorm => 77,
        BC3RgbaUnormSrgb => 78,
        BC4RUnorm => 80,
        BC4RSnorm => 81,
        BC5RgUnorm => 83,
        BC5RgSnorm => 84,
        BC7RgbaUnorm => 98,
        BC7RgbaUnormSrgb => 99,
        _ => return None,
    })
}

/// Legacy fourcc for the unorm BC1/BC2/BC3 formats that have one. sRGB and
/// BC4-7 have no legacy fourcc and are written as DX10.
fn legacy_fourcc(format: ImageFormat) -> Option<[u8; 4]> {
    Some(match format {
        ImageFormat::BC1RgbaUnorm => *b"DXT1",
        ImageFormat::BC2RgbaUnorm => *b"DXT3",
        ImageFormat::BC3RgbaUnorm => *b"DXT5",
        _ => return None,
    })
}

struct Parsed {
    format: ImageFormat,
    width: u32,
    height: u32,
    mipmaps: u32,
    data_start: usize,
}

fn parse(bytes: &[u8]) -> io::Result<Parsed> {
    if bytes.len() < 128 || &bytes[..4] != b"DDS " || u32_at(bytes, 4) != 124 {
        return Err(bad("not a DDS file"));
    }
    let height = u32_at(bytes, 12);
    let width = u32_at(bytes, 16);
    let depth = u32_at(bytes, 24);
    let mipmaps = u32_at(bytes, 28).max(1);
    if width == 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS {
        return Err(bad("unsupported DDS dimensions"));
    }
    if depth > 1 {
        return Err(bad("3D DDS volumes are unsupported"));
    }
    if mipmaps > 32 {
        return Err(bad("unsupported DDS mip count"));
    }
    if u32_at(bytes, 112) & (0xFE00 | 0x200000) != 0 {
        return Err(bad("cubemap/volume DDS is unsupported"));
    }
    if u32_at(bytes, 76) != 32 {
        return Err(bad("unsupported DDS pixel format"));
    }
    let pf_flags = u32_at(bytes, 80);
    let fourcc = &bytes[84..88];
    let (format, data_start) = if fourcc == b"DX10" {
        if bytes.len() < 148 {
            return Err(bad("truncated DDS DX10 header"));
        }
        if u32_at(bytes, 132) != 3 || u32_at(bytes, 136) != 0 || u32_at(bytes, 140) != 1 {
            return Err(bad("DDS DX10 arrays/cubemaps/volumes are unsupported"));
        }
        (dxgi_format(u32_at(bytes, 128)).ok_or_else(|| bad("unsupported DDS DXGI format"))?, 148)
    } else {
        match fourcc {
            b"DXT1" => (ImageFormat::BC1RgbaUnorm, 128),
            b"DXT3" => (ImageFormat::BC2RgbaUnorm, 128),
            b"DXT5" => (ImageFormat::BC3RgbaUnorm, 128),
            _ if pf_flags & 0x40 != 0 => {
                let bits = u32_at(bytes, 88);
                let masks = (u32_at(bytes, 92), u32_at(bytes, 96), u32_at(bytes, 100), u32_at(bytes, 104));
                match (bits, masks) {
                    (32, (0x00ff0000, 0x0000ff00, 0x000000ff, 0xff000000)) => (ImageFormat::Bgra8Unorm, 128),
                    (32, (0x000000ff, 0x0000ff00, 0x00ff0000, 0xff000000)) => (ImageFormat::Rgba8Unorm, 128),
                    _ => return Err(bad("unsupported DDS uncompressed layout")),
                }
            }
            _ => return Err(bad("unsupported DDS fourcc")),
        }
    };
    if bytes_per_pixel(format).is_none() && block_dims(format).is_none() {
        return Err(bad("unsupported DDS format"));
    }
    let total = chain_size(format, width, height, mipmaps).ok_or_else(|| bad("unsupported DDS format"))?;
    if bytes.len() - data_start != total as usize {
        return Err(bad("DDS payload length does not match its mip chain"));
    }
    Ok(Parsed { format, width, height, mipmaps, data_start })
}

/// Build a DDS container from an encoded surface. Legacy fourcc is used where it
/// exists; everything else is written as a DX10 2D texture.
fn build_dds<T: AsRef<[u8]>>(surface: &Surface<T>) -> io::Result<Vec<u8>> {
    let mip0 = mip_size(surface.image_format, surface.width, surface.height)
        .ok_or_else(|| bad("cannot size the encoded texture"))?;
    if mip0 > u32::MAX as u64 {
        return Err(bad("encoded base mip is too large for the DDS field"));
    }
    let mut out = Vec::new();
    let mut header = [0u8; 148];
    header[..4].copy_from_slice(b"DDS ");
    put_u32(&mut header, 4, 124);
    let flags = 0x1 | 0x2 | 0x4 | 0x1000 | 0x80000; // CAPS|HEIGHT|WIDTH|PIXELFORMAT|LINEARSIZE
    let caps = 0x1000 | if surface.mipmaps > 1 { 0x8 | 0x400000 } else { 0 };
    put_u32(&mut header, 8, flags);
    put_u32(&mut header, 12, surface.height);
    put_u32(&mut header, 16, surface.width);
    put_u32(&mut header, 20, mip0 as u32);
    put_u32(&mut header, 28, surface.mipmaps);
    put_u32(&mut header, 76, 32);
    put_u32(&mut header, 80, 0x4); // DDPF_FOURCC
    put_u32(&mut header, 108, caps);
    match legacy_fourcc(surface.image_format) {
        Some(fourcc) => header[84..88].copy_from_slice(&fourcc),
        None => {
            let dxgi = dxgi_of(surface.image_format).ok_or_else(|| bad("no DDS output encoding for this format"))?;
            header[84..88].copy_from_slice(b"DX10");
            put_u32(&mut header, 128, dxgi);
            put_u32(&mut header, 132, 3); // D3D10_RESOURCE_DIMENSION_TEXTURE2D
            put_u32(&mut header, 136, 0); // no cubemap
            put_u32(&mut header, 140, 1); // one array slice
            put_u32(&mut header, 144, 0);
            out.extend_from_slice(&header);
            out.extend_from_slice(surface.data.as_ref());
            return Ok(out);
        }
    }
    out.extend_from_slice(&header[..128]);
    out.extend_from_slice(surface.data.as_ref());
    Ok(out)
}

/// Preserve aspect ratio, never upscale, and never return a zero dimension.
fn scale_to(width: u32, height: u32, max_dim: u32) -> (u32, u32) {
    if width <= max_dim && height <= max_dim {
        return (width, height);
    }
    let scale = max_dim as f64 / width.max(height) as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

/// Downscale a DDS texture to `max_dim` and export it to `output`. The source is
/// only read; the codec is preserved and the mip chain is regenerated.
pub fn compress(max_dim: u32, input: &Path, output: &Path) -> io::Result<()> {
    if !(16..=16384).contains(&max_dim) {
        return Err(bad("texture maximum dimension must be 16..=16384"));
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(input)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() < 128 || before.len() > MAX_FILE {
        return Err(bad("texture input must be a regular file up to 512 MiB"));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    file.read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if before.len() != after.len() || before.mtime() != after.mtime() || before.mtime_nsec() != after.mtime_nsec() {
        return Err(bad("texture source changed during read"));
    }
    let parsed = parse(&bytes)?;
    let surface = Surface {
        width: parsed.width,
        height: parsed.height,
        depth: 1,
        layers: 1,
        mipmaps: parsed.mipmaps,
        image_format: parsed.format,
        data: &bytes[parsed.data_start..],
    };
    let decoded = surface.decode_rgba8().map_err(|error| bad(&format!("cannot decode DDS texture: {error}")))?;
    let base: RgbaImage = decoded.get_image(0, 0, 0).ok_or_else(|| bad("DDS texture has no base mip"))?;
    let (width, height) = base.dimensions();
    let (new_width, new_height) = scale_to(width, height, max_dim);
    let resized = if (new_width, new_height) == (width, height) {
        base
    } else {
        image::imageops::resize(&base, new_width, new_height, FilterType::Lanczos3)
    };
    let encoded = SurfaceRgba8::from_image(&resized)
        .encode(parsed.format, Quality::Normal, Mipmaps::GeneratedAutomatic)
        .map_err(|error| bad(&format!("cannot encode DDS texture: {error}")))?;
    let output_bytes = build_dds(&encoded)?;
    // Fail closed if the container we produced does not round-trip, and confirm
    // the written texture actually decodes back to the expected base mip.
    let check = parse(&output_bytes)?;
    if check.width != encoded.width
        || check.height != encoded.height
        || check.mipmaps != encoded.mipmaps
        || check.format != encoded.image_format
    {
        return Err(bad("internal texture verification failed"));
    }
    let roundtrip = Surface {
        width: check.width,
        height: check.height,
        depth: 1,
        layers: 1,
        mipmaps: check.mipmaps,
        image_format: check.format,
        data: &output_bytes[check.data_start..],
    }
    .decode_rgba8()
    .map_err(|error| bad(&format!("exported texture failed to decode: {error}")))?;
    let decoded_base = roundtrip.get_image(0, 0, 0).ok_or_else(|| bad("exported texture has no base mip"))?;
    if decoded_base.dimensions() != (encoded.width, encoded.height) {
        return Err(bad("exported texture base mip does not match its header"));
    }
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(output)?;
    destination.write_all(&output_bytes)?;
    destination.sync_all()?;
    println!(
        "TEXTURE_COMPRESS|{}|{}|{}|{}|{}|{}|{:?}|{}|{}",
        before.len(),
        output_bytes.len(),
        width,
        height,
        encoded.width,
        encoded.height,
        encoded.image_format,
        encoded.mipmaps,
        if resized_is_smaller(width, height, encoded.width, encoded.height) { "downscaled" } else { "reencoded" },
    );
    eprintln!("Exported a downscaled DDS copy. The codec is preserved, the payload is lossy and unverified in-game, and the source file is never modified.");
    Ok(())
}

fn resized_is_smaller(w: u32, h: u32, nw: u32, nh: u32) -> bool {
    nw < w || nh < h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    fn checkerboard(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            if (x / 4 + y / 4) % 2 == 0 {
                image::Rgba([200, 40, 90, 255])
            } else {
                image::Rgba([30, 160, 210, 200])
            }
        })
    }

    fn encoded_source(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let image = checkerboard(width, height);
        let surface = SurfaceRgba8::from_image(&image)
            .encode(format, Quality::Normal, Mipmaps::Disabled)
            .unwrap();
        build_dds(&surface).unwrap()
    }

    fn surface_of(bytes: &[u8]) -> Surface<&[u8]> {
        let parsed = parse(bytes).unwrap();
        Surface {
            width: parsed.width,
            height: parsed.height,
            depth: 1,
            layers: 1,
            mipmaps: parsed.mipmaps,
            image_format: parsed.format,
            data: &bytes[parsed.data_start..],
        }
    }

    #[test]
    fn block_and_pixel_size_math_is_consistent() {
        assert_eq!(mip_size(ImageFormat::BC1RgbaUnorm, 8, 8), Some(2 * 2 * 8));
        assert_eq!(mip_size(ImageFormat::BC3RgbaUnorm, 8, 8), Some(2 * 2 * 16));
        assert_eq!(mip_size(ImageFormat::BC7RgbaUnorm, 5, 5), Some(2 * 2 * 16));
        assert_eq!(mip_size(ImageFormat::Bgra8Unorm, 3, 2), Some(3 * 2 * 4));
        assert_eq!(chain_size(ImageFormat::BC3RgbaUnorm, 8, 8, 4), Some(2 * 2 * 16 + 1 * 1 * 16 * 3));
        assert_eq!(mip_size(ImageFormat::BC6hRgbUfloat, 4, 4), None);
    }

    #[test]
    fn legacy_bc3_source_parses_and_round_trips_through_the_container() {
        let source = encoded_source(16, 16, ImageFormat::BC3RgbaUnorm);
        let parsed = parse(&source).unwrap();
        assert_eq!((parsed.format, parsed.width, parsed.height, parsed.mipmaps), (ImageFormat::BC3RgbaUnorm, 16, 16, 1));
        let surface = surface_of(&source);
        let rebuilt = build_dds(&surface).unwrap();
        assert_eq!(rebuilt, source);
    }

    #[test]
    fn legacy_bgra_source_is_written_as_a_valid_dx10_texture() {
        let source = encoded_source(12, 8, ImageFormat::Bgra8Unorm);
        let parsed = parse(&source).unwrap();
        assert_eq!((parsed.format, parsed.width, parsed.height), (ImageFormat::Bgra8Unorm, 12, 8));
        let surface = surface_of(&source);
        let rebuilt = build_dds(&surface).unwrap();
        assert_eq!(parse(&rebuilt).unwrap().format, ImageFormat::Bgra8Unorm);
        assert_eq!(&rebuilt[84..88], b"DX10");
    }

    #[test]
    fn cubemaps_volumes_and_truncation_are_refused() {
        let mut source = encoded_source(8, 8, ImageFormat::BC1RgbaUnorm);
        source[112..116].copy_from_slice(&0x200u32.to_le_bytes()); // cubemap face bit
        assert!(parse(&source).is_err());
        let source = encoded_source(8, 8, ImageFormat::BC1RgbaUnorm);
        assert!(parse(&source[..100]).is_err());
        let mut short = source.clone();
        short.push(0);
        assert!(parse(&short).is_err());
    }

    #[test]
    fn compress_downscales_a_real_legacy_dxt5_file_without_touching_the_source() {
        let root = std::env::temp_dir().join(format!("bgc-texture-{}", stamp()));
        fs::create_dir(&root).unwrap();
        let input = root.join("input.dds");
        let output = root.join("output.dds");
        let original = encoded_source(64, 64, ImageFormat::BC3RgbaUnorm);
        fs::write(&input, &original).unwrap();
        compress(32, &input, &output).unwrap();
        let written = fs::read(&output).unwrap();
        let parsed = parse(&written).unwrap();
        assert_eq!((parsed.format, parsed.width, parsed.height), (ImageFormat::BC3RgbaUnorm, 32, 32));
        assert!(parsed.mipmaps >= 6);
        assert!(written.len() < original.len());
        assert_eq!(fs::read(&input).unwrap(), original, "source must be untouched");
        // Re-running against the existing destination must fail, not overwrite.
        assert!(compress(16, &input, &output).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scale_to_never_upscales_or_zeroes_a_dimension() {
        assert_eq!(scale_to(64, 32, 1920), (64, 32));
        assert_eq!(scale_to(3840, 2160, 1920), (1920, 1080));
        assert_eq!(scale_to(1, 4096, 64), (1, 64));
    }
}
