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

struct Outcome {
    before: u64,
    after: u64,
    width: u32,
    height: u32,
    new_width: u32,
    new_height: u32,
    format: ImageFormat,
    mipmaps: u32,
    downscaled: bool,
}

struct Prepared {
    outcome: Outcome,
    bytes: Vec<u8>,
}

/// For exact power-of-two reductions, retain the existing lower-resolution
/// mip bytes rather than decoding and lossy-reencoding them. This preserves the
/// *encoded* pixel data of all retained levels byte-for-byte, but lowering the
/// logical texture dimensions is still a lossy and potentially incompatible
/// change. Only use a fully parsed chain with an exact dimension match.
fn reuse_existing_mips(
    parsed: &Parsed,
    source: &[u8],
    target_w: u32,
    target_h: u32,
) -> io::Result<Option<Vec<u8>>> {
    if parsed.mipmaps < 2 || (target_w, target_h) == (parsed.width, parsed.height) {
        return Ok(None);
    }
    let (mut width, mut height) = (parsed.width, parsed.height);
    let mut start = parsed.data_start;
    for index in 0..parsed.mipmaps {
        if index > 0 && (width, height) == (target_w, target_h) {
            let surface = Surface {
                width,
                height,
                depth: 1,
                layers: 1,
                mipmaps: parsed.mipmaps - index,
                image_format: parsed.format,
                data: &source[start..],
            };
            let candidate = build_dds(&surface)?;
            if candidate.len() >= source.len() {
                return Ok(None);
            }
            let check = parse(&candidate)?;
            if check.width != width
                || check.height != height
                || check.mipmaps != parsed.mipmaps - index
                || check.format != parsed.format
                || candidate[check.data_start..] != source[start..]
            {
                return Err(bad("retained DDS mip data did not round-trip"));
            }
            return Ok(Some(candidate));
        }
        let size = mip_size(parsed.format, width, height)
            .ok_or_else(|| bad("cannot size source DDS mip"))? as usize;
        start = start.checked_add(size).ok_or_else(|| bad("DDS mip offset overflow"))?;
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    Ok(None)
}

/// Decode, downscale and re-encode one DDS texture in memory, then verify the
/// container. The source is only read; the codec is preserved and the mip chain
/// is regenerated. Nothing is written here.
fn prepare(max_dim: u32, input: &Path, guarded_apply: bool) -> io::Result<Prepared> {
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
    if guarded_apply {
        let (target_w, target_h) = scale_to(parsed.width, parsed.height, max_dim);
        // Fail before allocating and decoding large BCn pixel surfaces.
        if (target_w, target_h) == (parsed.width, parsed.height) {
            return Err(bad("texture fits this profile; skip lossy re-encoding"));
        }
        if crate::texture_policy::output_too_thin(target_w, target_h) {
            return Err(bad("DDS downscale would create a too-thin texture"));
        }
    }
    let (target_w, target_h) = scale_to(parsed.width, parsed.height, max_dim);
    if let Some(retained) = reuse_existing_mips(&parsed, &bytes, target_w, target_h)? {
        let check = parse(&retained)?;
        return Ok(Prepared {
            outcome: Outcome {
                before: before.len(),
                after: retained.len() as u64,
                width: parsed.width,
                height: parsed.height,
                new_width: check.width,
                new_height: check.height,
                format: check.format,
                mipmaps: check.mipmaps,
                downscaled: true,
            },
            bytes: retained,
        });
    }
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
    // Only the explicit detached exporter allows at-cap or too-thin re-encoding.
    // Installed apply candidates passed their quality gates before decoding.
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
    Ok(Prepared {
        outcome: Outcome {
            before: before.len(),
            after: output_bytes.len() as u64,
            width,
            height,
            new_width: encoded.width,
            new_height: encoded.height,
            format: encoded.image_format,
            mipmaps: encoded.mipmaps,
            downscaled: encoded.width < width || encoded.height < height,
        },
        bytes: output_bytes,
    })
}

/// Write an export to a new file. An existing destination is refused.
fn write_output(output: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut destination = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(output)?;
    destination.write_all(bytes)?;
    destination.sync_all()?;
    Ok(())
}

/// Asset-pipeline entry point: downscale a supported DDS to `max_edge` and
/// return the re-encoded bytes plus the source dimensions only when the result
/// is strictly smaller. The source is never modified. Unsupported layouts return
/// `Ok(None)` rather than an error so a library scan can continue.
pub fn prepare_asset(max_edge: u32, path: &Path) -> io::Result<Option<(Vec<u8>, u32, u32)>> {
    if !(16..=16384).contains(&max_edge) {
        return Ok(None);
    }
    match prepare(max_edge, path, true) {
        Ok(prepared) if prepared.outcome.downscaled && prepared.outcome.after < prepared.outcome.before => {
            Ok(Some((prepared.bytes, prepared.outcome.width, prepared.outcome.height)))
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Ok(None),
        Err(error) => Err(error),
    }
}

/// Read-only diagnostic: compare a candidate DDS against an original downscaled
/// to the same dimensions. This measures codec-generation differences versus a
/// Lanczos3 reference; it deliberately does NOT measure detail lost by lowering
/// the resolution or establish game/runtime compatibility.
#[derive(Debug)]
struct QualityMetrics {
    original_w: u32,
    original_h: u32,
    candidate_w: u32,
    candidate_h: u32,
    psnr_black: f64,
    psnr_white: f64,
    alpha_differing_pixels: u64,
}

fn read_quality_input(path: &Path) -> io::Result<(Vec<u8>, Parsed)> {
    const QUALITY_MAX_BYTES: u64 = 128 * 1024 * 1024;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() < 128 || before.len() > QUALITY_MAX_BYTES {
        return Err(bad("quality analysis requires a regular DDS up to 128 MiB"));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    // Bound the actual bytes read as well as the initial fstat size, even if
    // another process grows the descriptor after the metadata check.
    (&mut file).take(QUALITY_MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > QUALITY_MAX_BYTES {
        return Err(bad("DDS grew past quality analysis file budget"));
    }
    let after = file.metadata()?;
    if before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
    {
        return Err(bad("DDS changed during quality analysis"));
    }
    let parsed = parse(&bytes)?;
    if parsed.width as u64 * parsed.height as u64 > 16_777_216 {
        return Err(bad("quality analysis exceeds the 16 million pixel budget"));
    }
    Ok((bytes, parsed))
}

fn decode_quality_base(bytes: &[u8], p: &Parsed) -> io::Result<RgbaImage> {
    let surface = Surface {
        width: p.width,
        height: p.height,
        depth: 1,
        layers: 1,
        mipmaps: p.mipmaps,
        image_format: p.format,
        data: &bytes[p.data_start..],
    };
    let pixels = surface.decode_rgba8()
        .map_err(|error| bad(&format!("cannot decode quality DDS: {error}")))?;
    pixels.get_image(0, 0, 0).ok_or_else(|| bad("quality DDS has no base mip"))
}

fn quality_metrics(original_path: &Path, candidate_path: &Path) -> io::Result<QualityMetrics> {
    let (original_bytes, original) = read_quality_input(original_path)?;
    let (candidate_bytes, candidate) = read_quality_input(candidate_path)?;
    if original.format != candidate.format
        || candidate.width > original.width
        || candidate.height > original.height
    {
        return Err(bad("candidate DDS must retain the codec and never upscale"));
    }
    let source = decode_quality_base(&original_bytes, &original)?;
    let expected = if (original.width, original.height) == (candidate.width, candidate.height) {
        source
    } else {
        image::imageops::resize(
            &source, candidate.width, candidate.height, FilterType::Lanczos3,
        )
    };
    let actual = decode_quality_base(&candidate_bytes, &candidate)?;
    if expected.dimensions() != actual.dimensions() {
        return Err(bad("decoded quality DDS dimensions disagree"));
    }
    let (mut black_error, mut white_error, mut alpha_differing) = (0f64, 0f64, 0u64);
    for (a, b) in expected.pixels().zip(actual.pixels()) {
        let ap = a.0;
        let bp = b.0;
        if ap[3] != bp[3] { alpha_differing += 1; }
        for channel in 0..3 {
            let av = ap[channel] as f64 * ap[3] as f64 / 255.0;
            let bv = bp[channel] as f64 * bp[3] as f64 / 255.0;
            black_error += (av - bv).powi(2);
            // Also compare the appearance when alpha is composited onto white.
            let aw = av + (255 - ap[3]) as f64;
            let bw = bv + (255 - bp[3]) as f64;
            white_error += (aw - bw).powi(2);
        }
    }
    let samples = candidate.width as f64 * candidate.height as f64 * 3.0;
    let psnr = |error: f64| {
        if error == 0.0 { f64::INFINITY }
        else { 10.0 * (255.0f64 * 255.0 * samples / error).log10() }
    };
    Ok(QualityMetrics {
        original_w: original.width,
        original_h: original.height,
        candidate_w: candidate.width,
        candidate_h: candidate.height,
        psnr_black: psnr(black_error),
        psnr_white: psnr(white_error),
        alpha_differing_pixels: alpha_differing,
    })
}

/// Compare two DDS files without writing either source. The PSNR values
/// describe only additional codec distortion after matching dimensions.
pub fn quality(original: &Path, candidate: &Path) -> io::Result<()> {
    let q = quality_metrics(original, candidate)?;
    println!(
        "TEXTURE_QUALITY|{}|{}|{}|{}|{:.2}|{:.2}|{}",
        q.original_w, q.original_h, q.candidate_w, q.candidate_h,
        q.psnr_black, q.psnr_white, q.alpha_differing_pixels,
    );
    eprintln!("Quality comparison is against a resized source, not the original rendered size. It does not measure lost detail, engine compatibility or in-game quality.");
    Ok(())
}

/// Downscale a DDS texture to `max_dim` and export it to `output`, printing one
/// machine-readable summary line. The source is only read.
pub fn compress(max_dim: u32, input: &Path, output: &Path) -> io::Result<()> {
    if !(16..=16384).contains(&max_dim) {
        return Err(bad("texture maximum dimension must be 16..=16384"));
    }
    let prepared = prepare(max_dim, input, false)?;
    write_output(output, &prepared.bytes)?;
    let outcome = prepared.outcome;
    println!(
        "TEXTURE_COMPRESS|{}|{}|{}|{}|{}|{}|{:?}|{}|{}",
        outcome.before,
        outcome.after,
        outcome.width,
        outcome.height,
        outcome.new_width,
        outcome.new_height,
        outcome.format,
        outcome.mipmaps,
        if outcome.downscaled { "downscaled" } else { "reencoded" },
    );
    eprintln!("Exported a downscaled DDS copy. The codec is preserved, the payload is lossy and unverified in-game, and the source file is never modified.");
    Ok(())
}

/// Export-only tree pass: mirror every DDS under `input` into `output`,
/// downscaling each to `max_dim`. Unsupported or refused textures are skipped and
/// other failures are counted; the source tree is never modified.
pub fn compress_tree(max_dim: u32, input: &Path, output: &Path) -> io::Result<()> {
    if !(16..=16384).contains(&max_dim) {
        return Err(bad("texture maximum dimension must be 16..=16384"));
    }
    if !fs::symlink_metadata(input)?.file_type().is_dir() {
        return Err(bad("texture-compress-tree expects a real directory"));
    }
    let mut directories = vec![input.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)? {
            super::cancelled()?;
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("dds"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    let (mut exported, mut skipped, mut failed) = (0u64, 0u64, 0u64);
    let (mut before_bytes, mut after_bytes) = (0u64, 0u64);
    for path in files {
        super::cancelled()?;
        let Ok(relative) = path.strip_prefix(input) else { continue };
        let destination = output.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        match prepare(max_dim, &path, false) {
            Ok(prepared) => {
                // Export only when the rebuilt texture is actually smaller. Adding
                // a mip chain to an already-small single-mip texture would grow it.
                if prepared.outcome.after >= prepared.outcome.before {
                    skipped += 1;
                    println!("TEXTURE_COMPRESS_FILE|{}|skipped_no_gain", relative.display());
                } else if write_output(&destination, &prepared.bytes).is_err() {
                    failed += 1;
                    println!("TEXTURE_COMPRESS_FILE|{}|failed", relative.display());
                } else {
                    exported += 1;
                    before_bytes = before_bytes.saturating_add(prepared.outcome.before);
                    after_bytes = after_bytes.saturating_add(prepared.outcome.after);
                    println!("TEXTURE_COMPRESS_FILE|{}|{}|{}|exported", relative.display(), prepared.outcome.before, prepared.outcome.after);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                skipped += 1;
                println!("TEXTURE_COMPRESS_FILE|{}|skipped_unsupported", relative.display());
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                skipped += 1;
                println!("TEXTURE_COMPRESS_FILE|{}|skipped_existing", relative.display());
            }
            Err(_) => {
                failed += 1;
                println!("TEXTURE_COMPRESS_FILE|{}|failed", relative.display());
            }
        }
    }
    println!("TEXTURE_COMPRESS_TOTAL|{exported}|{before_bytes}|{after_bytes}|{skipped}|{failed}");
    eprintln!("Export-only tree pass: {exported} DDS file(s) downscaled into a new tree; the source tree is untouched, {skipped} unsupported/refused and {failed} failed. Payloads are lossy and unverified in-game.");
    Ok(())
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

    #[test]
    fn tree_pass_mirrors_supported_dds_and_skips_the_rest() {        let root = std::env::temp_dir().join(format!("bgc-texture-tree-{}", stamp()));
        let input = root.join("in");
        let output = root.join("out");
        fs::create_dir_all(input.join("nested")).unwrap();
        let dxt5 = encoded_source(64, 64, ImageFormat::BC3RgbaUnorm);
        fs::write(input.join("a.dds"), &dxt5).unwrap();
        fs::write(input.join("nested/b.dds"), &dxt5).unwrap();
        fs::write(input.join("nested/ignore.txt"), b"not a texture").unwrap();
        fs::write(input.join("broken.dds"), b"not a dds").unwrap();
        compress_tree(16, &input, &output).unwrap();
        assert!(output.join("a.dds").exists());
        assert!(output.join("nested/b.dds").exists());
        assert!(!output.join("nested/ignore.txt").exists());
        assert!(!output.join("broken.dds").exists());
        assert_eq!(fs::read(input.join("a.dds")).unwrap(), dxt5, "source tree must be untouched");
        // A second pass must not overwrite existing exports.
        let first = fs::read(output.join("a.dds")).unwrap();
        compress_tree(16, &input, &output).unwrap();
        assert_eq!(fs::read(output.join("a.dds")).unwrap(), first, "existing export must not be overwritten");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepare_asset_gates_on_real_reduction_for_dx10_and_legacy() {
        let root = std::env::temp_dir().join(format!("bgc-texture-asset-{}", stamp()));
        fs::create_dir(&root).unwrap();
        // A large DX10 BC7 source is rewritten smaller when downscaled.
        let big = root.join("big.dds");
        fs::write(&big, encoded_source(1024, 1024, ImageFormat::BC7RgbaUnorm)).unwrap();
        let got = prepare_asset(256, &big).unwrap();
        assert!(got.is_some(), "a large DX10 BC7 texture should be a candidate");
        let (bytes, width, height) = got.unwrap();
        assert_eq!((width, height), (1024, 1024));
        assert_eq!(parse(&bytes).unwrap().format, ImageFormat::BC7RgbaUnorm);
        // A small legacy texture must not grow just to gain a mip chain.
        let small = root.join("small.dds");
        fs::write(&small, encoded_source(64, 64, ImageFormat::BC3RgbaUnorm)).unwrap();
        assert!(prepare_asset(512, &small).unwrap().is_none());
        // Non-DDS and unsupported inputs are simply not candidates.
        let other = root.join("other.bin");
        fs::write(&other, b"not a texture").unwrap();
        assert!(prepare_asset(512, &other).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installed_dds_apply_skips_thin_results_but_export_still_works() {
        let root = std::env::temp_dir().join(format!("bgc-texture-thin-{}", stamp()));
        fs::create_dir(&root).unwrap();
        let source = root.join("source.dds");
        let destination = root.join("export.dds");
        fs::write(&source, encoded_source(1024, 128, ImageFormat::BC3RgbaUnorm)).unwrap();
        assert!(prepare_asset(256, &source).unwrap().is_none(), "256x32 is too thin for installed apply");
        compress(256, &source, &destination).unwrap();
        let exported = fs::read(&destination).unwrap();
        let p = parse(&exported).unwrap();
        assert_eq!((p.width, p.height), (256, 32));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installed_dds_apply_skips_already_at_cap() {
        let root = std::env::temp_dir().join(format!("bgc-texture-at-cap-{}", stamp()));
        fs::create_dir(&root).unwrap();
        let source = root.join("source.dds");
        let original = encoded_source(512, 512, ImageFormat::BC7RgbaUnorm);
        fs::write(&source, &original).unwrap();
        assert!(prepare_asset(640, &source).unwrap().is_none());
        assert_eq!(fs::read(&source).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_mip_reduction_reuses_the_encoded_mip_tail() {
        let root = std::env::temp_dir().join(format!("bgc-texture-mip-reuse-{}", stamp()));
        fs::create_dir(&root).unwrap();
        let path = root.join("source.dds");
        let image = checkerboard(64, 64);
        let encoded = SurfaceRgba8::from_image(&image)
            .encode(ImageFormat::BC3RgbaUnorm, Quality::Normal, Mipmaps::GeneratedAutomatic)
            .unwrap();
        let original = build_dds(&encoded).unwrap();
        let before = parse(&original).unwrap();
        assert!(before.mipmaps >= 2);
        fs::write(&path, &original).unwrap();
        let prepared = prepare(32, &path, false).unwrap();
        let after = parse(&prepared.bytes).unwrap();
        assert_eq!((after.width, after.height), (32, 32));
        assert_eq!(after.mipmaps, before.mipmaps - 1);
        let first_mip_size = mip_size(before.format, 64, 64).unwrap() as usize;
        assert_eq!(
            &prepared.bytes[after.data_start..],
            &original[before.data_start + first_mip_size..],
            "the retained compressed mip bytes must be identical"
        );
        assert_eq!(fs::read(&path).unwrap(), original, "source is read-only");
        // A non-mip-aligned target must keep the normal resampling path.
        let interpolated = prepare(48, &path, false).unwrap();
        assert_eq!(parse(&interpolated.bytes).unwrap().width, 48);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quality_diagnostic_is_read_only_and_rejects_incompatible_candidates() {
        let root = std::env::temp_dir().join(format!("bgc-texture-quality-{}", stamp()));
        fs::create_dir(&root).unwrap();
        let original_path = root.join("original.dds");
        let candidate_path = root.join("candidate.dds");
        let original = encoded_source(64, 64, ImageFormat::BC3RgbaUnorm);
        fs::write(&original_path, &original).unwrap();
        compress(32, &original_path, &candidate_path).unwrap();
        let candidate_before = fs::read(&candidate_path).unwrap();
        let metrics = quality_metrics(&original_path, &candidate_path).unwrap();
        assert_eq!((metrics.original_w, metrics.original_h), (64, 64));
        assert_eq!((metrics.candidate_w, metrics.candidate_h), (32, 32));
        assert!(metrics.psnr_black > 0.0 && metrics.psnr_white > 0.0);
        assert!(metrics.alpha_differing_pixels <= 32 * 32);
        quality(&original_path, &candidate_path).unwrap();
        assert_eq!(fs::read(&original_path).unwrap(), original);
        assert_eq!(fs::read(&candidate_path).unwrap(), candidate_before);
        assert!(quality_metrics(&candidate_path, &original_path).is_err());
        fs::write(root.join("bad.dds"), b"not a DDS").unwrap();
        assert!(quality_metrics(&original_path, &root.join("bad.dds")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn re_preparing_an_already_processed_texture_is_a_no_op() {
        let root = std::env::temp_dir().join(format!("bgc-texture-idem-{}", stamp()));
        fs::create_dir(&root).unwrap();
        let source = root.join("source.dds");
        fs::write(&source, encoded_source(1024, 1024, ImageFormat::BC7RgbaUnorm)).unwrap();
        let (bytes, _, _) = prepare_asset(256, &source).unwrap().expect("large texture is a candidate");
        let processed = root.join("processed.dds");
        fs::write(&processed, &bytes).unwrap();
        // A second pass at the same or a larger cap must not shrink it again.
        assert!(prepare_asset(256, &processed).unwrap().is_none(), "second pass at the same cap must be a no-op");
        assert!(prepare_asset(1024, &processed).unwrap().is_none(), "a larger cap must not upscale or re-shorten");
        assert_eq!(fs::read(&processed).unwrap(), bytes, "the processed file must be unchanged");
        fs::remove_dir_all(root).unwrap();
    }
}
