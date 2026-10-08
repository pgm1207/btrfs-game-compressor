//! Beta, profile-aware optimization of loose raster images, DDS textures and
//! supported integer/float WAV audio. The codecs are
//! statically linked into bgc-native; no external image program is executed.
use image::{
    codecs::{
        jpeg::JpegEncoder,
        png::{CompressionType, FilterType as PngFilter, PngEncoder},
        webp::WebPEncoder,
    },
    AnimationDecoder, DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
};
use std::{
    collections::BTreeMap,
    fs::{self, File, FileTimes, OpenOptions},
    io::{self, BufReader, Cursor, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const BACKUP: &str = ".bgc-assets-backup";
const NOFOLLOW: i32 = libc::O_NOFOLLOW;
const MAX_INPUT: u64 = 512 * 1024 * 1024;
const MAX_PIXELS: u64 = 80_000_000;

struct Profile {
    label: &'static str,
    max_edge: u32,
    jpeg_quality: u8,
    color_bits: u8,
    audio_rate: u32,
    audio_bits: u16,
    audio_target: Option<crate::audio_policy::Target>,
}
fn profile(s: &str) -> io::Result<Option<Profile>> {
    let mut p = match s {
        "ultra-performance" => Profile { label:"Ultra Performance (480p; WAV up to 11.025 kHz / 8-bit)", max_edge:640, jpeg_quality:60, color_bits:4, audio_rate:11025, audio_bits:8, audio_target:None },
        "performance" => Profile { label:"Performance (720p)", max_edge:1280, jpeg_quality:80, color_bits:6, audio_rate:32000, audio_bits:16, audio_target:None },
        "balanced" => Profile { label:"Balanced (1080p)", max_edge:1920, jpeg_quality:86, color_bits:7, audio_rate:44100, audio_bits:16, audio_target:None },
        "quality" => Profile { label:"Quality (1440p)", max_edge:2560, jpeg_quality:91, color_bits:8, audio_rate:48000, audio_bits:16, audio_target:None },
        "ultra-quality" => Profile { label:"Ultra Quality (4K)", max_edge:3840, jpeg_quality:95, color_bits:8, audio_rate:48000, audio_bits:16, audio_target:None },
        "lossless" => Profile { label:"Lossless (Hades packages only)", max_edge:u32::MAX, jpeg_quality:100, color_bits:8, audio_rate:u32::MAX, audio_bits:u16::MAX, audio_target:None },
        "native" => return Ok(None),
        _ => return Err(bad("visual target must be ultra-performance, performance, balanced, quality, ultra-quality, lossless, or native")),
    };
    p.audio_target = crate::audio_policy::target_for(s)?;
    if let Some(audio) = p.audio_target {
        p.audio_rate = audio.pcm_rate;
        p.audio_bits = audio.pcm_bits;
    }
    Ok(Some(p))
}
fn bad(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn append_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut name = p.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    p.with_file_name(name)
}
fn open_read(p: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).custom_flags(NOFOLLOW).open(p)
}
fn clone_file(src: &Path, dest: &Path) -> io::Result<()> {
    let source = open_read(src)?;
    let target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(NOFOLLOW)
        .open(dest)?;
    // Linux FICLONE: retain original blocks cheaply on Btrfs for later restore.
    let rc = unsafe {
        super::ioctl(
            target.as_raw_fd(),
            super::request(1, 0x94, 9, 4),
            source.as_raw_fd(),
        )
    };
    if rc < 0 {
        // Reflinks are a Btrfs (and XFS) feature. Fall back to a real copy so
        // the workflow also works on other filesystems and in tests; the
        // backup then costs full space instead of sharing blocks.
        let _ = fs::remove_file(dest);
        let mut from = open_read(src)?;
        let mut to = OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(NOFOLLOW)
            .open(dest)?;
        let result = (|| {
            io::copy(&mut from, &mut to)?;
            let m = from.metadata()?;
            to.set_permissions(m.permissions())?;
            to.set_times(FileTimes::new().set_modified(m.modified()?))?;
            to.sync_all()
        })();
        if result.is_err() {
            let _ = fs::remove_file(dest);
        }
        return result;
    }
    let result = (|| {
        let m = source.metadata()?;
        target.set_permissions(m.permissions())?;
        target.set_times(FileTimes::new().set_modified(m.modified()?))?;
        target.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}
fn hash(data: &[u8]) -> u64 {
    hash_update(0xcbf29ce484222325u64, data)
}
fn hash_update(mut h: u64, data: &[u8]) -> u64 {
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
fn sidecar(backup: &Path) -> PathBuf {
    append_suffix(backup, ".bgc-checksum")
}
fn matches_expected(path: &Path, side: &Path) -> io::Result<bool> {
    let expected = match open_read(side) {
        Ok(mut f) => {
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            s
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut f = open_read(path)?;
    let mut digest = 0xcbf29ce484222325u64;
    let mut buffer = [0u8; 65536];
    loop {
        super::cancelled()?;
        let n = f.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest = hash_update(digest, &buffer[..n]);
    }
    Ok(expected.trim() == format!("{digest:016x}"))
}
// Match the existing sidecar checksum format while streaming large packs.
fn checksum_file(path: &Path) -> io::Result<u64> {
    let mut input = open_read(path)?;
    let mut checksum = 0xcbf29ce484222325u64;
    let mut buffer = [0u8; 65536];
    loop {
        super::cancelled()?;
        let n = input.read(&mut buffer)?;
        if n == 0 { break; }
        checksum = hash_update(checksum, &buffer[..n]);
    }
    Ok(checksum)
}

fn same_contents(a: &Path, b: &Path) -> io::Result<bool> {
    // A pruned asset is intentionally absent from the game tree, so a missing
    // live file is never "the same" and never an error here.
    let mut a = match open_read(a) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut b = open_read(b)?;
    if a.metadata()?.len() != b.metadata()?.len() {
        return Ok(false);
    }
    let (mut ab, mut bb) = ([0u8; 65536], [0u8; 65536]);
    loop {
        super::cancelled()?;
        let n = a.read(&mut ab)?;
        if n == 0 {
            return Ok(true);
        }
        b.read_exact(&mut bb[..n])?;
        if ab[..n] != bb[..n] {
            return Ok(false);
        }
    }
}
// Sidecar contents that mark a backup as a deliberate removal rather than a
// transformed file. The live path is expected to be absent in that state.
const REMOVED: &[u8] = b"removed\n";
fn is_removal(backup: &Path) -> io::Result<bool> {
    match open_read(&sidecar(backup)) {
        Ok(mut f) => {
            let mut s = String::new();
            f.read_to_string(&mut s)?;
            Ok(s == "removed\n")
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}
fn known_version(path: &Path, backup: &Path) -> io::Result<bool> {
    if is_removal(backup)? {
        return Ok(!path.exists());
    }
    Ok(same_contents(path, backup)? || matches_expected(path, &sidecar(backup))?)
}
fn ensure_backup_dirs(root: &Path, relative_parent: &Path) -> io::Result<PathBuf> {
    let mut p = root.join(BACKUP);
    if !p.exists() {
        fs::create_dir(&p)?;
    }
    if !fs::symlink_metadata(&p)?.file_type().is_dir() {
        return Err(bad("asset backup path is not a real directory"));
    }
    for c in relative_parent.components() {
        p.push(c);
        if !p.exists() {
            fs::create_dir(&p)?;
        }
        if !fs::symlink_metadata(&p)?.file_type().is_dir() {
            return Err(bad("asset backup path is not a real directory"));
        }
    }
    Ok(p)
}
fn format_for(path: &Path) -> Option<ImageFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::WebP),
        "gif" => Some(ImageFormat::Gif),
        "dds" => Some(ImageFormat::Dds),
        "bmp" => Some(ImageFormat::Bmp),
        "tga" => Some(ImageFormat::Tga),
        "qoi" => Some(ImageFormat::Qoi),
        "pnm" => Some(ImageFormat::Pnm),
        _ => None,
    }
}
fn may_have_raster_signature(path: &Path) -> bool {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    {
        None => true,
        Some(_) => false,
    }
}
fn ancillary_metadata_safe(path: &Path, format: ImageFormat) -> io::Result<bool> {
    if format == ImageFormat::Dds {
        return dds_header(path).map(|h| h.is_some());
    }
    if format == ImageFormat::Gif {
        let input = BufReader::new(open_read(path)?);
        let decoder = match image::codecs::gif::GifDecoder::new(input) {
            Ok(d) => d,
            Err(_) => return Ok(false),
        };
        let mut frames = decoder.into_frames();
        if !matches!(frames.next(), Some(Ok(_))) {
            return Ok(false);
        }
        // Only single-frame GIFs can be resized without silently deleting
        // animation frames from the game.
        return Ok(frames.next().is_none());
    }
    let mut f = open_read(path)?;
    let file_len = f.metadata()?.len();
    if format == ImageFormat::Png {
        let mut sig = [0u8; 8];
        f.read_exact(&mut sig)?;
        if sig != *b"\x89PNG\r\n\x1a\n" {
            return Ok(false);
        }
        loop {
            let mut head = [0u8; 8];
            f.read_exact(&mut head)?;
            let size = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
            let kind = &head[4..8];
            // Keep pixel transparency (tRNS is decoded into alpha); skip PNGs
            // whose color, density, EXIF or animation metadata would be lost.
            if kind[0] & 0x20 != 0 && kind != b"tRNS" {
                return Ok(false);
            }
            if f.stream_position()?.saturating_add(size).saturating_add(4) > file_len {
                return Ok(false);
            }
            f.seek(SeekFrom::Current(size as i64 + 4))?;
            if kind == b"IEND" {
                return Ok(f.stream_position()? == file_len);
            }
        }
    }
    if format == ImageFormat::WebP {
        let mut riff = [0u8; 12];
        f.read_exact(&mut riff)?;
        if &riff[..4] != b"RIFF" || &riff[8..] != b"WEBP" {
            return Ok(false);
        }
        if u32::from_le_bytes(riff[4..8].try_into().unwrap()) as u64 + 8 != file_len {
            return Ok(false);
        }
        loop {
            let mut head = [0u8; 8];
            match f.read_exact(&mut head) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(f.stream_position()? == file_len)
                }
                Err(e) => return Err(e),
            }
            let size = u32::from_le_bytes(head[4..8].try_into().unwrap()) as u64;
            if matches!(&head[..4], b"ANIM" | b"ANMF" | b"ICCP" | b"EXIF" | b"XMP ") {
                return Ok(false);
            }
            if f.stream_position()?
                .saturating_add(size)
                .saturating_add(size & 1)
                > file_len
            {
                return Ok(false);
            }
            f.seek(SeekFrom::Current(size as i64 + (size & 1) as i64))?;
        }
    }
    if format == ImageFormat::Jpeg {
        if file_len < 2 {
            return Ok(false);
        }
        f.seek(SeekFrom::End(-2))?;
        let mut end = [0u8; 2];
        f.read_exact(&mut end)?;
        return Ok(end == [0xff, 0xd9]);
    }
    if format == ImageFormat::Qoi {
        if file_len < 8 {
            return Ok(false);
        }
        f.seek(SeekFrom::End(-8))?;
        let mut end = [0u8; 8];
        f.read_exact(&mut end)?;
        return Ok(end == [0, 0, 0, 0, 0, 0, 0, 1]);
    }
    Ok(true)
}

#[derive(Clone, Copy)]
enum DdsKind {
    Bc1,
    Bc2,
    Bc3,
}
fn dds_header(path: &Path) -> io::Result<Option<([u8; 128], DdsKind)>> {
    let mut f = open_read(path)?;
    let mut h = [0u8; 128];
    if f.read_exact(&mut h).is_err()
        || &h[..4] != b"DDS "
        || u32::from_le_bytes(h[4..8].try_into().unwrap()) != 124
    {
        return Ok(None);
    }
    // Only standalone legacy 2D BC1/2/3 textures are rewritten. DX10 arrays,
    // cubemaps, volumes and other GPU codecs need separate format handling.
    if u32::from_le_bytes(h[80..84].try_into().unwrap()) & 4 == 0 {
        return Ok(None);
    }
    let caps2 = u32::from_le_bytes(h[112..116].try_into().unwrap());
    if caps2 & (0xFE00 | 0x200000) != 0 || u32::from_le_bytes(h[28..32].try_into().unwrap()) > 1 {
        return Ok(None);
    }
    let kind = match &h[84..88] {
        b"DXT1" => DdsKind::Bc1,
        b"DXT3" => DdsKind::Bc2,
        b"DXT5" => DdsKind::Bc3,
        _ => return Ok(None),
    };
    let w = u32::from_le_bytes(h[16..20].try_into().unwrap()) as u64;
    let height = u32::from_le_bytes(h[12..16].try_into().unwrap()) as u64;
    let block_bytes = if matches!(kind, DdsKind::Bc1) { 8 } else { 16 };
    if w == 0 || height == 0 || w * height > MAX_PIXELS {
        return Ok(None);
    }
    let expected = 128 + w.div_ceil(4) * height.div_ceil(4) * block_bytes;
    if f.metadata()?.len() != expected {
        return Ok(None);
    }
    // BC1 can encode one-bit transparency in three-color mode. Our simple
    // encoder emits opaque four-color blocks, so leave alpha-bearing textures
    // alone instead of silently turning cutouts solid.
    if matches!(kind, DdsKind::Bc1) {
        f.seek(SeekFrom::Start(128))?;
        let mut block = [0u8; 8];
        let blocks = ((w + 3) / 4) * ((height + 3) / 4);
        for _ in 0..blocks {
            f.read_exact(&mut block)?;
            let c0 = u16::from_le_bytes(block[0..2].try_into().unwrap());
            let c1 = u16::from_le_bytes(block[2..4].try_into().unwrap());
            let selectors = u32::from_le_bytes(block[4..8].try_into().unwrap());
            if c0 <= c1 && (0..16).any(|i| ((selectors >> (i * 2)) & 3) == 3) {
                return Ok(None);
            }
        }
    }
    Ok(Some((h, kind)))
}
fn rgb565(c: [u8; 4]) -> u16 {
    ((u16::from(c[0]) * 31 / 255) << 11)
        | ((u16::from(c[1]) * 63 / 255) << 5)
        | (u16::from(c[2]) * 31 / 255)
}
fn unpack565(v: u16) -> [u8; 3] {
    let r = (v >> 11) & 31;
    let g = (v >> 5) & 63;
    let b = v & 31;
    [
        (r * 255 / 31) as u8,
        (g * 255 / 63) as u8,
        (b * 255 / 31) as u8,
    ]
}
fn color_block(pixels: &[[u8; 4]; 16]) -> [u8; 8] {
    let mut lo = [255u8; 3];
    let mut hi = [0u8; 3];
    for p in pixels {
        for c in 0..3 {
            lo[c] = lo[c].min(p[c]);
            hi[c] = hi[c].max(p[c]);
        }
    }
    let mut c0 = rgb565([hi[0], hi[1], hi[2], 255]);
    let mut c1 = rgb565([lo[0], lo[1], lo[2], 255]);
    if c0 <= c1 {
        if c0 < u16::MAX {
            c0 = c1.saturating_add(1);
        } else {
            c1 = c0.saturating_sub(1);
        }
    }
    let a = unpack565(c0);
    let b = unpack565(c1);
    let palette = [
        a,
        b,
        [
            ((2 * a[0] as u16 + b[0] as u16) / 3) as u8,
            ((2 * a[1] as u16 + b[1] as u16) / 3) as u8,
            ((2 * a[2] as u16 + b[2] as u16) / 3) as u8,
        ],
        [
            ((a[0] as u16 + 2 * b[0] as u16) / 3) as u8,
            ((a[1] as u16 + 2 * b[1] as u16) / 3) as u8,
            ((a[2] as u16 + 2 * b[2] as u16) / 3) as u8,
        ],
    ];
    let mut selectors = 0u32;
    for (i, p) in pixels.iter().enumerate() {
        let mut best = (u32::MAX, 0u32);
        for (j, q) in palette.iter().enumerate() {
            let d = (0..3)
                .map(|c| (i32::from(p[c]) - i32::from(q[c])).pow(2) as u32)
                .sum();
            if d < best.0 {
                best = (d, j as u32);
            }
        }
        selectors |= best.1 << (2 * i);
    }
    let mut out = [0u8; 8];
    out[..2].copy_from_slice(&c0.to_le_bytes());
    out[2..4].copy_from_slice(&c1.to_le_bytes());
    out[4..].copy_from_slice(&selectors.to_le_bytes());
    out
}
fn alpha_block_dxt5(pixels: &[[u8; 4]; 16]) -> [u8; 8] {
    let lo = pixels.iter().map(|p| p[3]).min().unwrap_or(0);
    let hi = pixels.iter().map(|p| p[3]).max().unwrap_or(255);
    let mut palette = [0u8; 8];
    palette[0] = hi;
    palette[1] = lo;
    if hi > lo {
        for i in 2..8 {
            palette[i] = (((8 - i) as u16 * hi as u16 + (i - 1) as u16 * lo as u16) / 7) as u8;
        }
    } else {
        palette[2..].fill(hi);
    }
    let mut bits = 0u64;
    for (i, p) in pixels.iter().enumerate() {
        let best = (0..8)
            .min_by_key(|&j| (i16::from(p[3]) - i16::from(palette[j])).abs())
            .unwrap_or(0);
        bits |= (best as u64) << (3 * i);
    }
    let mut out = [0u8; 8];
    out[0] = hi;
    out[1] = lo;
    out[2..].copy_from_slice(&bits.to_le_bytes()[..6]);
    out
}
fn encode_dds(img: &DynamicImage, path: &Path) -> io::Result<Vec<u8>> {
    let Some((mut header, kind)) = dds_header(path)? else {
        return Err(bad("unsupported DDS texture layout"));
    };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w < 4 || h < 4 {
        return Err(bad("DDS output dimensions must be at least 4x4"));
    }
    let block_bytes = if matches!(kind, DdsKind::Bc1) {
        8usize
    } else {
        16usize
    };
    let blocks_x = w.div_ceil(4) as usize;
    let blocks_y = h.div_ceil(4) as usize;
    let mut out = Vec::with_capacity(128 + blocks_x * blocks_y * block_bytes);
    out.extend_from_slice(&header);
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let mut px = [[0u8; 4]; 16];
            for y in 0..4 {
                for x in 0..4 {
                    px[y * 4 + x] = rgba
                        .get_pixel(
                            ((bx * 4 + x) as u32).min(w - 1),
                            ((by * 4 + y) as u32).min(h - 1),
                        )
                        .0;
                }
            }
            match kind {
                DdsKind::Bc1 => out.extend_from_slice(&color_block(&px)),
                DdsKind::Bc2 => {
                    let mut alpha = 0u64;
                    for (i, p) in px.iter().enumerate() {
                        alpha |= ((u64::from(p[3]) * 15 / 255) & 15) << (4 * i);
                    }
                    out.extend_from_slice(&alpha.to_le_bytes());
                    out.extend_from_slice(&color_block(&px));
                }
                DdsKind::Bc3 => {
                    out.extend_from_slice(&alpha_block_dxt5(&px));
                    out.extend_from_slice(&color_block(&px));
                }
            }
        }
    }
    let flags = 0x1 | 0x2 | 0x4 | 0x1000 | 0x80000;
    header[8..12].copy_from_slice(&flagsu32(flags));
    header[12..16].copy_from_slice(&h.to_le_bytes());
    header[16..20].copy_from_slice(&w.to_le_bytes());
    header[20..24].copy_from_slice(&((blocks_x * blocks_y * block_bytes) as u32).to_le_bytes());
    header[24..32].fill(0);
    header[108..112].copy_from_slice(&0x1000u32.to_le_bytes());
    header[112..116].fill(0);
    out[..128].copy_from_slice(&header);
    Ok(out)
}
fn flagsu32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}
fn encode(
    img: &DynamicImage,
    format: ImageFormat,
    profile: &Profile,
    source: &Path,
) -> io::Result<Vec<u8>> {
    if format == ImageFormat::Dds {
        return encode_dds(img, source);
    }
    let mut out = Cursor::new(Vec::new());
    let result = match format {
        ImageFormat::Jpeg => {
            JpegEncoder::new_with_quality(&mut out, profile.jpeg_quality).encode_image(img)
        }
        ImageFormat::Png => {
            PngEncoder::new_with_quality(&mut out, CompressionType::Best, PngFilter::Adaptive)
                .write_image(
                    img.as_bytes(),
                    img.width(),
                    img.height(),
                    img.color().into(),
                )
        }
        ImageFormat::WebP => WebPEncoder::new_lossless(&mut out).write_image(
            img.as_bytes(),
            img.width(),
            img.height(),
            img.color().into(),
        ),
        f => img.write_to(&mut out, f),
    };
    result.map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("image encode failed: {e}"),
        )
    })?;
    Ok(out.into_inner())
}
fn quantize_channel(value: u8, bits: u8) -> u8 {
    if bits >= 8 { return value; }
    let levels = (1u32 << bits) - 1;
    let quantized = (u32::from(value) * levels + 127) / 255;
    ((quantized * 255 + levels / 2) / levels) as u8
}
fn reduce_rgb_precision(img: &mut DynamicImage, bits: u8) {
    match img {
        DynamicImage::ImageRgb8(pixels) if bits < 8 => {
            for pixel in pixels.pixels_mut() { for channel in &mut pixel.0 { *channel = quantize_channel(*channel, bits); } }
        }
        DynamicImage::ImageRgba8(pixels) if bits < 8 => {
            for pixel in pixels.pixels_mut() { for channel in &mut pixel.0[..3] { *channel = quantize_channel(*channel, bits); } }
        }
        _ => {},
    }
}

fn read_pcm_sample(s: &[u8], bits: u16) -> i32 {
    match bits {
        8 => i32::from(s[0]) - 128,
        16 => i16::from_le_bytes([s[0], s[1]]) as i32,
        24 => {
            let v = i32::from(s[0]) | (i32::from(s[1]) << 8) | (i32::from(s[2]) << 16);
            (v << 8) >> 8
        }
        32 => i32::from_le_bytes(s[..4].try_into().unwrap()),
        _ => 0,
    }
}
fn read_wav_normalized(s: &[u8], bits: u16, float_pcm: bool) -> Option<f64> {
    if float_pcm {
        let value = f32::from_le_bytes(s[..4].try_into().ok()?) as f64;
        value.is_finite().then(|| value.clamp(-1.0, 1.0))
    } else {
        Some(f64::from(read_pcm_sample(s, bits)) / 2f64.powi(i32::from(bits - 1)))
    }
}
fn write_pcm_sample(out: &mut Vec<u8>, normalized: f64, bits: u16, dither: f64) {
    let scale = 2f64.powi(i32::from(bits - 1));
    let value = normalized * scale + dither;
    match bits {
        8 => out.push((value.round() as i32 + 128).clamp(0, 255) as u8),
        16 => out.extend_from_slice(
            &(value.round() as i32)
                .clamp(i16::MIN as i32, i16::MAX as i32)
                .to_le_bytes()[..2],
        ),
        24 => {
            let v = (value.round() as i32).clamp(-8_388_608, 8_388_607);
            out.extend_from_slice(&v.to_le_bytes()[..3]);
        }
        32 => out.extend_from_slice(
            &(value.round() as i64)
                .clamp(i32::MIN as i64, i32::MAX as i64)
                .to_le_bytes()[..4],
        ),
        _ => {}
    }
}
fn tpdf_dither(index: u64) -> f64 {
    fn uniform(mut x: u64) -> f64 {
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        ((x.wrapping_mul(0x2545f4914f6cdd1d) >> 11) as f64) / 9_007_199_254_740_992.0
    }
    uniform(index.wrapping_add(0x9e3779b97f4a7c15)) - uniform(index.wrapping_add(0xd1b54a32d192ed03))
}
fn resample_wav(path: &Path, p: &Profile) -> io::Result<Option<Vec<u8>>> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() || meta.nlink() != 1 || meta.len() < 44 || meta.len() > MAX_INPUT
    {
        return Ok(None);
    }
    let mut f = open_read(path)?;
    let file_len = meta.len();
    let mut root = [0u8; 12];
    f.read_exact(&mut root)?;
    if &root[..4] != b"RIFF"
        || &root[8..] != b"WAVE"
        || u32::from_le_bytes(root[4..8].try_into().unwrap()) as u64 + 8 != file_len
    {
        return Ok(None);
    }
    let mut pos = 12u64;
    let mut fmt = None;
    let mut data = None;
    while pos + 8 <= file_len {
        f.seek(SeekFrom::Start(pos))?;
        let mut ch = [0u8; 8];
        f.read_exact(&mut ch)?;
        let size = u32::from_le_bytes(ch[4..8].try_into().unwrap()) as u64;
        let start = pos + 8;
        let end = start
            .checked_add(size)
            .ok_or_else(|| bad("WAV chunk overflow"))?;
        if end > file_len {
            return Ok(None);
        }
        match &ch[..4] {
            b"fmt " if fmt.is_none() && size == 16 => {
                let mut b = [0u8; 16];
                f.read_exact(&mut b)?;
                fmt = Some(b);
            }
            b"data" if data.is_none() => data = Some((start, size)),
            _ => return Ok(None), // Unknown chunks may contain timing/loop metadata.
        }
        pos = end + (size & 1);
    }
    if pos != file_len {
        return Ok(None);
    }
    let Some(fmt) = fmt else {
        return Ok(None);
    };
    let Some((data_at, data_size)) = data else {
        return Ok(None);
    };
    let tag = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
    let channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
    let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
    let align = u16::from_le_bytes(fmt[12..14].try_into().unwrap());
    let bits = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
    let float_pcm = tag == 3 && bits == 32;
    if !(tag == 1 || float_pcm)
        || !(channels == 1 || channels == 2)
        || (!float_pcm && !matches!(bits, 8 | 16 | 24 | 32))
        || rate == 0
    {
        return Ok(None);
    }
    let output_rate = rate.min(p.audio_rate);
    let output_bits = bits.min(p.audio_bits);
    if output_rate == rate && output_bits == bits { return Ok(None); }
    let sample_bytes = (bits / 8) as u64;
    let expected_align = u64::from(channels) * sample_bytes;
    if u64::from(align) != expected_align || data_size == 0 || data_size % expected_align != 0 {
        return Ok(None);
    }
    let frames = data_size / expected_align;
    let out_frames = ((frames as u128 * u128::from(output_rate) + u128::from(rate) / 2)
        / u128::from(rate)) as usize;
    if out_frames == 0 {
        return Ok(None);
    }
    f.seek(SeekFrom::Start(data_at))?;
    let mut src = vec![0u8; data_size as usize];
    f.read_exact(&mut src)?;
    let source_frames = frames as usize;
    let channels = channels as usize;
    let bytes_per = (bits / 8) as usize;
    let output_bytes_per = usize::from(output_bits / 8);
    let output_align = u16::try_from(channels * output_bytes_per).map_err(|_| bad("WAV output alignment overflow"))?;
    let mut audio = Vec::with_capacity(out_frames * channels * output_bytes_per);
    let cutoff = output_rate as f64 / rate as f64;
    let radius = 16.0 / cutoff;
    for o in 0..out_frames {
        if o % 16_384 == 0 {
            super::cancelled()?;
        }
        let center = if output_rate == rate { o as f64 } else { o as f64 / cutoff };
        let first = if output_rate == rate { o as isize } else { (center - radius).floor() as isize };
        let last = if output_rate == rate { o as isize } else { (center + radius).ceil() as isize };
        for c in 0..channels {
            let mut sum = 0.0;
            let mut weight_sum = 0.0;
            for ix in first..=last {
                let clamped = ix.clamp(0, source_frames as isize - 1) as usize;
                if output_rate == rate {
                    let start = (clamped * channels + c) * bytes_per;
                    let Some(sample) = read_wav_normalized(&src[start..start + bytes_per], bits, float_pcm) else { return Ok(None); };
                    sum = sample;
                    weight_sum = 1.0;
                    break;
                }
                let d = ix as f64 - center;
                let x = d * cutoff;
                let sinc = if x.abs() < 1e-9 {
                    1.0
                } else {
                    (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                };
                let window = if d.abs() >= radius {
                    0.0
                } else {
                    0.5 + 0.5 * (std::f64::consts::PI * d / radius).cos()
                };
                let weight = cutoff * sinc * window;
                let start = (clamped * channels + c) * bytes_per;
                let Some(sample) = read_wav_normalized(&src[start..start + bytes_per], bits, float_pcm) else { return Ok(None); };
                sum += sample * weight;
                weight_sum += weight;
            }
            let sample = if weight_sum.abs() > 1e-12 {
                sum / weight_sum
            } else {
                0.0
            };
            let dither = if output_bits < bits { tpdf_dither((o as u64).wrapping_mul(channels as u64).wrapping_add(c as u64)) } else { 0.0 };
            write_pcm_sample(&mut audio, sample, output_bits, dither);
        }
    }
    if audio.len() >= data_size as usize {
        return Ok(None);
    }
    let byte_rate = output_rate
        .checked_mul(u32::from(output_align))
        .ok_or_else(|| bad("WAV byte rate overflow"))?;
    let riff_size = 4u64 + 8 + 16 + 8 + audio.len() as u64 + (audio.len() as u64 & 1);
    if riff_size > u32::MAX as u64 || audio.len() > u32::MAX as usize {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(riff_size as usize + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(riff_size as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(channels as u16).to_le_bytes());
    out.extend_from_slice(&output_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&output_align.to_le_bytes());
    out.extend_from_slice(&output_bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(audio.len() as u32).to_le_bytes());
    out.extend_from_slice(&audio);
    if audio.len() & 1 != 0 {
        out.push(0);
    }
    Ok(Some(out))
}

/// Inventory report mode: walk regular files and container paths without
/// decoding images/audio or rebuilding packed candidates. Counts are inventory,
/// never claimed savings. This keeps whole-library opportunity scans bounded.
pub fn inventory(root: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(bad("game path is not a real directory"));
    }
    let mut files = 0u64;
    let mut logical_bytes = 0u64;
    let mut raster_files = 0u64;
    let mut packed_media = 0u64;
    let mut packed_bytes = 0u64;
    let mut audio_files = 0u64;
    let mut audio_bytes = 0u64;
    let dev = fs::metadata(root)?.dev();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        super::cancelled()?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == BACKUP) { continue; }
            let meta = fs::symlink_metadata(&path)?;
            if meta.dev() != dev { continue; }
            if meta.file_type().is_dir() { dirs.push(path); continue; }
            if !meta.file_type().is_file() { continue; }
            files += 1;
            logical_bytes = logical_bytes.saturating_add(meta.len());
            match path.extension().and_then(|ext| ext.to_str()).map(str::to_ascii_lowercase).as_deref() {
                Some("dds" | "ktx" | "ktx2" | "astc" | "pvr" | "crn" | "vtex" | "tex" | "texture"
                    | "assets" | "bundle" | "unity3d" | "pak" | "pck" | "wad" | "resource"
                    | "resources" | "pkg" | "xnb" | "bik" | "bk2" | "mp4" | "webm" | "ogv") => {
                        packed_media += 1; packed_bytes = packed_bytes.saturating_add(meta.len());
                    }
                Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tga" | "qoi" | "pnm") => raster_files += 1,
                Some("wav" | "flac" | "ogg" | "mp3" | "m4a" | "aac" | "opus" | "wem" | "bank" | "bnk" | "fsb") => {
                    audio_files += 1; audio_bytes = audio_bytes.saturating_add(meta.len());
                }
                _ => {}
            }
        }
    }
    println!("ASSET_INVENTORY|{files}|{logical_bytes}|{raster_files}|{packed_media}|{packed_bytes}|{audio_files}|{audio_bytes}");
    Ok(())
}

/// Lossless inventory of bounded, standalone containers with relevant writers.
/// This identifies validation targets; it does not claim an item is transformable.
pub fn container_inventory(root: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(bad("game path is not a real directory"));
    }
    let dev = fs::metadata(root)?.dev();
    let mut dirs = vec![root.to_path_buf()];
    let mut rows: BTreeMap<String, (u64, u64, PathBuf)> = BTreeMap::new();
    while let Some(dir) = dirs.pop() {
        super::cancelled()?;
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == BACKUP) { continue; }
            let meta = fs::symlink_metadata(&path)?;
            if meta.dev() != dev { continue; }
            if meta.file_type().is_dir() { dirs.push(path); continue; }
            if !meta.file_type().is_file() { continue; }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let class = match ext.as_str() {
                "pck" => {
                    let mut f = open_read(&path)?;
                    let mut magic = [0u8; 4];
                    if f.read_exact(&mut magic).is_err() || &magic != b"GDPC" { continue; }
                    "godot-pck"
                }
                "bank" | "fsb" => "fmod-audio",
                "pkg" => "hades-pkg",
                _ => continue,
            };
            let rel = path.strip_prefix(root).map_err(|_| bad("container escaped game directory"))?.to_owned();
            let row = rows.entry(class.to_owned()).or_insert((0, 0, rel));
            row.0 += 1;
            row.1 = row.1.saturating_add(meta.len());
        }
    }
    for (class, (count, bytes, example)) in rows {
        println!("ASSET_CONTAINER|{class}|{count}|{bytes}|{}", example.display());
    }
    Ok(())
}

pub fn audit_standalone_fmod(root: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(bad("game path is not a real directory"));
    }
    let dev = fs::metadata(root)?.dev();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        super::cancelled()?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == BACKUP) { continue; }
            let meta = fs::symlink_metadata(&path)?;
            if meta.dev() != dev { continue; }
            if meta.file_type().is_dir() { dirs.push(path); continue; }
            if !meta.file_type().is_file() || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("bank") || e.eq_ignore_ascii_case("fsb")) { continue; }
            let rel = path.strip_prefix(root).map_err(|_| bad("FMOD path escaped game directory"))?;
            match super::fmod::audit(&path) {
                Ok(()) => println!("ASSET_FMOD_AUDIT|OK|{}|{}", meta.len(), rel.display()),
                Err(e) => println!("ASSET_FMOD_AUDIT|REJECT|{}|{}|{}", meta.len(), rel.display(), e.to_string().replace('|', "/")),
            }
        }
    }
    Ok(())
}

/// Print bounded format metadata for the standalone PCKs in one game tree.
pub fn audit_packs(root: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(bad("game path is not a real directory"));
    }
    let dev = fs::metadata(root)?.dev();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        super::cancelled()?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == BACKUP) { continue; }
            let meta = fs::symlink_metadata(&path)?;
            if meta.dev() != dev { continue; }
            if meta.file_type().is_dir() { dirs.push(path); continue; }
            if !meta.file_type().is_file() || !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pck")) { continue; }
            let mut f = open_read(&path)?;
            let mut magic = [0u8; 4];
            if f.read_exact(&mut magic).is_err() || &magic != b"GDPC" { continue; }
            let rel = path.strip_prefix(root).map_err(|_| bad("PCK escaped game directory"))?;
            let before = meta.len();
            let mut header = [0u8; 20];
            f.seek(SeekFrom::Start(0))?;
            f.read_exact(&mut header)?;
            let version = u32::from_le_bytes(header[4..8].try_into().unwrap());
            let major = u32::from_le_bytes(header[8..12].try_into().unwrap());
            let minor = u32::from_le_bytes(header[12..16].try_into().unwrap());
            let patch = u32::from_le_bytes(header[16..20].try_into().unwrap());
            let directory_offset = pck_directory_offset(&mut f, version)?;
            let declared = pck_directory_count(&mut f, before, directory_offset)?;
            let audit = super::containers::godot_audit(&path);
            match audit {
                Ok(()) => println!("ASSET_PACK_AUDIT|OK|{before}|{version}|{major}.{minor}.{patch}|{declared}|{}", rel.display()),
                Err(e) => println!("ASSET_PACK_AUDIT|REJECT|{before}|{version}|{major}.{minor}.{patch}|{declared}|{}|{}", rel.display(), e.to_string().replace('|', "/")),
            }
        }
    }
    Ok(())
}

pub fn trial_apply_packs(root: &Path) -> io::Result<()> {
    run("apply", "balanced", 1, root)
}

fn pck_directory_offset<R: Read + Seek>(file: &mut R, version: u32) -> io::Result<u64> {
    match version {
        1 => Ok(84),
        2 => Ok(96),
        3 | 4 => {
            file.seek(SeekFrom::Start(32))?;
            let mut field = [0u8; 8];
            file.read_exact(&mut field)?;
            Ok(u64::from_le_bytes(field))
        }
        _ => Err(bad("unsupported Godot PCK version")),
    }
}

fn pck_directory_count<R: Read + Seek>(file: &mut R, length: u64, offset: u64) -> io::Result<u64> {
    if offset.checked_add(4).filter(|end| *end <= length).is_none() {
        return Err(bad("Godot PCK directory offset outside file"));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut count = [0u8; 4];
    file.read_exact(&mut count)?;
    Ok(u64::from(u32::from_le_bytes(count)))
}


/// Apply the already implemented Balanced Godot texture writer to a disposable
/// tree via its normal pipeline. Callers must explicitly provide a copy.
pub fn apply_packs(root: &Path, level: u8) -> io::Result<()> {
    run("apply", "balanced", level, root)
}

fn prepare(path: &Path, p: &Profile) -> io::Result<Option<Vec<u8>>> {
    if crate::texture_policy::atlas_hint(path) { return Ok(None); }
    let meta = fs::symlink_metadata(path)?;
    if !meta.file_type().is_file() || meta.nlink() != 1 || meta.len() == 0 || meta.len() > MAX_INPUT
    {
        return Ok(None);
    }
    let extension_format = format_for(path);
    if extension_format.is_none() && !may_have_raster_signature(path) {
        return Ok(None);
    }
    // Check the format header and dimensions before walking metadata chunks or
    // decoding pixels. Most game images are already below the selected limit;
    // avoiding full metadata scans for those files makes library scans much
    // cheaper.
    let reader = ImageReader::open(path)?
        .with_guessed_format()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let detected_format = reader.format();
    let Some(format) = extension_format.or(detected_format) else {
        return Ok(None);
    };
    if detected_format.is_some_and(|detected| detected != format) {
        return Ok(None);
    }
    // Route DDS through the richer decoder/encoder: it handles legacy BC1/2/3
    // and 32-bit BGRA/RGBA plus DX10 BC1-7/RGBA/BGRA, rebuilds a complete mip
    // chain, and returns output only when it is strictly smaller. The generic
    // image decoder cannot read DX10/BC7 DDS.
    if format == ImageFormat::Dds {
        if crate::texture_policy::atlas_hint(path) {
            return Ok(None);
        }
        // `texture::prepare_asset` validates the DDS layout itself (legacy or
        // DX10, including multi-mip), so the legacy-only ancillary check and the
        // generic image decoder are both bypassed here.
        return match crate::texture::prepare_asset(p.max_edge, path)? {
            Some((bytes, width, height)) if !crate::texture_policy::small_or_thin(width, height) => Ok(Some(bytes)),
            _ => Ok(None),
        };
    }
    if !matches!(
        format,
        ImageFormat::Png
            | ImageFormat::Jpeg
            | ImageFormat::WebP
            | ImageFormat::Gif
            | ImageFormat::Dds
            | ImageFormat::Bmp
            | ImageFormat::Tga
            | ImageFormat::Qoi
            | ImageFormat::Pnm
    ) {
        return Ok(None);
    }
    let (header_w, header_h) = match reader.into_dimensions() {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };
    if header_w == 0
        || header_h == 0
        || header_w as u64 * header_h as u64 > MAX_PIXELS
        || header_w.max(header_h) <= p.max_edge
        || crate::texture_policy::small_or_thin(header_w, header_h)
    {
        return Ok(None);
    }
    if !ancillary_metadata_safe(path, format)? {
        return Ok(None);
    }
    let mut reader = ImageReader::open(path)?
        .with_guessed_format()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    reader.set_format(format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(32768);
    limits.max_image_height = Some(32768);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    if reader.format() != Some(format) {
        return Ok(None);
    }
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 || w as u64 * h as u64 > MAX_PIXELS || w.max(h) <= p.max_edge {
        return Ok(None);
    }
    // Preserve embedded color profiles when codecs expose them; skip a tagged
    // image rather than silently changing its color interpretation.
    let icc = decoder
        .icc_profile()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let orientation = decoder
        .orientation()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if icc.is_some() {
        return Ok(None);
    }
    if orientation != image::metadata::Orientation::NoTransforms {
        return Ok(None);
    }
    let img = DynamicImage::from_decoder(decoder)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let edge = crate::texture_policy::effective_edge(p.max_edge, img.width(), img.height());
    let scale = (edge as f64 / img.width().max(img.height()) as f64).min(1.0);
    let align = if format == ImageFormat::Dds { 4 } else { 1 };
    let nw = (((img.width() as f64 * scale).round() as u32 / align) * align).max(align);
    let nh = (((img.height() as f64 * scale).round() as u32 / align) * align).max(align);
    if crate::texture_policy::output_too_thin(nw, nh) { return Ok(None); }
    let mut resized = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
    // JPEG and BCn have their own lossy encoders. For other supported 8-bit
    // RGB rasters, progressively quantize color after scaling; alpha is exact.
    if !matches!(format, ImageFormat::Jpeg | ImageFormat::Dds) {
        reduce_rgb_precision(&mut resized, crate::texture_policy::color_bits(p.color_bits));
    }
    let output = encode(&resized, format, p, path)?;
    if output.len() >= meta.len() as usize {
        return Ok(None);
    }
    Ok(Some(output))
}
#[derive(Clone, Copy)]
enum AssetKind {
    Image,
    Texture,
    Audio,
    Package,
}
#[derive(Debug, Default)]
struct Inventory {
    packed_media: u64,
    audio_files: u64,
    raster_files: u64,
    logical_bytes: u64,
    packed_bytes: u64,
    audio_bytes: u64,
}

fn is_unity_tree_container(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(e.to_ascii_lowercase().as_str(), "assets" | "bundle" | "unity3d" | "resource" | "resources" | "ress")
    })
}

fn collect(
    root: &Path,
    p: &Profile,
    prepare_candidates: bool,
    mut visit: impl FnMut(PathBuf, Vec<u8>, AssetKind) -> io::Result<()>,
) -> io::Result<Inventory> {
    let dev = fs::metadata(root)?.dev();
    let mut dirs = vec![root.to_path_buf()];
    let mut inventory = Inventory::default();
    let mut visited = 0u64;
    let mut last_progress = Instant::now();
    let show_progress = std::env::var_os("BGC_ASSET_PROGRESS").is_some();
    let mut progress_entries = 0u64;
    while let Some(dir) = dirs.pop() {
        super::cancelled()?;
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == BACKUP) {
                continue;
            }
            let m = fs::symlink_metadata(&path)?;
            visited += 1;
            progress_entries += 1;
            if show_progress && progress_entries >= 8192 && last_progress.elapsed() >= Duration::from_secs(2) {
                eprintln!("Asset scan: visited {visited} entries; {} known loose image files, {} packed/media containers, {} audio files identified.", inventory.raster_files, inventory.packed_media, inventory.audio_files);
                last_progress = Instant::now();
                progress_entries = 0;
            }
            if m.dev() != dev {
                continue;
            }
            if m.file_type().is_dir() {
                dirs.push(path);
                continue;
            }
            if !m.file_type().is_file() {
                continue;
            }
            inventory.logical_bytes += m.len();
            match path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some(
                    "dds" | "ktx" | "ktx2" | "astc" | "pvr" | "crn" | "vtex" | "tex" | "texture"
                    | "assets" | "bundle" | "unity3d" | "pak" | "pck" | "wad" | "resource"
                    | "resources" | "pkg" | "xnb" | "bik" | "bk2" | "mp4" | "webm" | "ogv",
                ) => {
                    inventory.packed_media += 1;
                    inventory.packed_bytes += m.len();
                }
                Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tga" | "qoi" | "pnm") => inventory.raster_files += 1,
                Some(
                    "wav" | "flac" | "ogg" | "mp3" | "m4a" | "aac" | "opus" | "wem" | "bank"
                    | "bnk" | "fsb",
                ) => {
                    inventory.audio_files += 1;
                    inventory.audio_bytes += m.len();
                }
                _ => {}
            }
            let rel = path
                .strip_prefix(root)
                .map_err(|_| bad("asset escaped game directory"))?;
            if root.join(BACKUP).join(rel).exists() {
                continue;
            }
            if !prepare_candidates || is_unity_tree_container(&path) { continue; }
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("fsb") || e.eq_ignore_ascii_case("bank")) {
                // Unlike embedded .resource slices, standalone FMOD banks can
                // be rebuilt without touching Unity/Unreal serialized offsets.
                // Keep per-file work bounded; no unsafe signature scanning.
                if m.len() <= 256 * 1024 * 1024 {
                    match super::fmod::prepare(&path, p.audio_target) {
                        Ok(Some(new)) => visit(path, new, AssetKind::Audio)?,
                        Ok(None) => {},
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
                        Err(e) => {
                            if std::env::var_os("BGC_VERBOSE").is_some() {
                                eprintln!("Skipping standalone FMOD audio {}: {e}", path.display());
                            }
                        }
                    }
                }
                continue;
            }
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pkg")) {
                match super::packages::prepare(&path) {
                    Ok(Some(new)) => visit(path, new, AssetKind::Package)?,
                    Ok(None) => {}
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
                    Err(e) => {
                        if std::env::var_os("BGC_VERBOSE").is_some() {
                            eprintln!("Skipping package {}: {e}", path.display());
                        }
                    }
                }
                continue;
            }
            if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
            {
                match resample_wav(&path, p) {
                    Ok(Some(new)) => visit(path, new, AssetKind::Audio)?,
                    Ok(None) => {}
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
                    Err(e) if std::env::var_os("BGC_VERBOSE").is_some() => {
                        eprintln!("Skipping WAV {}: {e}", path.display())
                    }
                    Err(_) => {}
                }
                continue;
            }
            match prepare(&path, p) {
                Ok(Some(new)) => {
                    let kind = if path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("dds"))
                    {
                        AssetKind::Texture
                    } else {
                        AssetKind::Image
                    };
                    visit(path, new, kind)?;
                }
                Ok(None) => {}
                Err(e) => {
                    if e.kind() == io::ErrorKind::Interrupted {
                        return Err(e);
                    }
                    if std::env::var_os("BGC_VERBOSE").is_some() {
                        eprintln!("Skipping image {}: {e}", path.display());
                    }
                }
            }
        }
    }
    Ok(inventory)
}
fn walk_backups(root: &Path) -> io::Result<Vec<PathBuf>> {
    let base = root.join(BACKUP);
    if !base.exists() {
        return Ok(Vec::new());
    }
    if !fs::symlink_metadata(&base)?.file_type().is_dir() {
        return Err(bad("asset backup path is not a real directory"));
    }
    let mut dirs = vec![base.clone()];
    let dev = fs::symlink_metadata(root)?.dev();
    let mut out = Vec::new();
    while let Some(dir) = dirs.pop() {
        for e in fs::read_dir(&dir)? {
            let e = e?;
            let m = fs::symlink_metadata(e.path())?;
            if m.dev() != dev {
                return Err(bad("asset backup path crosses a mount"));
            }
            if m.file_type().is_dir() {
                dirs.push(e.path());
            } else if m.file_type().is_file()
                && !e.file_name().to_string_lossy().ends_with(".bgc-checksum")
            {
                out.push(e.path());
            }
        }
    }
    out.sort();
    Ok(out)
}
fn original_path(root: &Path, backup: &Path) -> io::Result<PathBuf> {
    let relative = backup
        .strip_prefix(root.join(BACKUP))
        .map_err(|_| bad("backup path escaped game"))?;
    let dev = fs::symlink_metadata(root)?.dev();
    let mut parent = root.to_path_buf();
    for component in relative.parent().unwrap_or(Path::new("")).components() {
        parent.push(component);
        let m = fs::symlink_metadata(&parent)?;
        if !m.file_type().is_dir() || m.dev() != dev {
            return Err(bad("asset restore path crosses a symlink or mount"));
        }
    }
    Ok(root.join(relative))
}
/// Privileged, read-only selection report. NUL-delimited path/original logical
/// bytes/candidate logical bytes records are emitted only after all checks pass.
pub fn physical_rejections(root: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let tree = super::Tree::new(root)?;
    let mut rejected = Vec::new();
    for backup in walk_backups(root)? {
        super::cancelled()?;
        if backup.extension().is_none_or(|ext| ext != "pkg") {
            return Err(bad("physical package selection requires a package-only backup set"));
        }
        let original = original_path(root,&backup)?;
        if !known_version(&original,&backup)? {
            return Err(bad("asset changed since optimization; refusing physical selection"));
        }
        let old = tree.open(backup.strip_prefix(root).map_err(|_| bad("backup escaped game"))?,false)?;
        let new = tree.open(original.strip_prefix(root).map_err(|_| bad("asset escaped game"))?,false)?;
        let mut old_sizes = super::Sizes::default();
        let mut new_sizes = super::Sizes::default();
        super::add_file_sizes(&old,&mut old_sizes)?;
        super::add_file_sizes(&new,&mut new_sizes)?;
        if new_sizes.disk >= old_sizes.disk {
            let relative = backup.strip_prefix(root.join(BACKUP)).map_err(|_| bad("backup escaped directory"))?.to_path_buf();
            rejected.push((relative,old.metadata()?.len(),new.metadata()?.len()));
        }
    }
    let mut out = io::stdout().lock();
    for (path,old,new) in rejected {
        out.write_all(path.as_os_str().as_bytes())?;
        write!(out,"\0{old}\0{new}\0")?;
    }
    Ok(())
}

pub fn restore_file(root: &Path, relative: &Path) -> io::Result<()> {
    if relative.as_os_str().is_empty() || relative.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return Err(bad("restore-file requires a normal game-relative path"));
    }
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(bad("game path is not a real directory"));
    }
    let backup = root.join(BACKUP).join(relative);
    let backups = walk_backups(root)?;
    if !backups.contains(&backup) { return Err(bad("no matching regular asset backup")); }
    let original = original_path(root,&backup)?;
    if !known_version(&original,&backup)? {
        return Err(bad("asset changed since optimization; refusing selective restore"));
    }
    finish_backups("restore",root,vec![backup])
}

// --- Removal of unused, non-gameplay content -------------------------------
//
// Two opt-in categories are recognized, both restorable through the same
// backup tree used by asset transforms:
//   * developer debug symbols (`.pdb`, `.ilk`) accidentally shipped with a
//     release build, and
//   * explicit game-relative paths (e.g. an unused low-resolution texture or
//     video fallback suite) supplied by the front end.
// Only regular files are ever selected. Symlinks, non-regular targets and
// paths that escape the game directory are rejected. Directories are left in
// place (possibly empty) so restoration never needs to recreate them.

fn debug_symbol(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "pdb" | "ilk"))
}

fn prune_files(root: &Path, rels: &[PathBuf], debug: bool) -> io::Result<Vec<PathBuf>> {
    let dev = fs::symlink_metadata(root)?.dev();
    let mut files = Vec::new();
    for rel in rels {
        if rel.as_os_str().is_empty()
            || rel.is_absolute()
            || rel
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(bad("prune paths must be non-empty, relative and free of '..'"));
        }
        let target = root.join(rel);
        let m = fs::symlink_metadata(&target)?;
        if m.file_type().is_symlink() {
            return Err(bad("prune target is a symlink"));
        }
        if m.file_type().is_file() {
            if m.dev() != dev {
                return Err(bad("prune target crosses a mount"));
            }
            files.push(target);
            continue;
        }
        if !m.file_type().is_dir() {
            return Err(bad("prune target is neither a file nor a directory"));
        }
        let mut stack = vec![target];
        while let Some(dir) = stack.pop() {
            super::cancelled()?;
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let m = match fs::symlink_metadata(entry.path()) {
                    Ok(m) => m,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                };
                if m.file_type().is_symlink() {
                    return Err(bad("prune directory contains a symlink"));
                }
                if m.dev() != dev {
                    return Err(bad("prune directory crosses a mount"));
                }
                if m.file_type().is_dir() {
                    stack.push(entry.path());
                } else if m.file_type().is_file() {
                    files.push(entry.path());
                } else {
                    return Err(bad("prune directory contains an unsupported entry"));
                }
            }
        }
    }
    if debug {
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            super::cancelled()?;
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let m = match fs::symlink_metadata(entry.path()) {
                    Ok(m) => m,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                };
                if m.file_type().is_symlink() || m.dev() != dev {
                    continue;
                }
                if m.file_type().is_dir() {
                    if entry.file_name() != BACKUP {
                        stack.push(entry.path());
                    }
                } else if m.file_type().is_file() && debug_symbol(&entry.path()) {
                    files.push(entry.path());
                }
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn prune_measure(files: &[PathBuf]) -> io::Result<u64> {
    let mut bytes = 0u64;
    for path in files {
        bytes += fs::symlink_metadata(path)?.len();
    }
    Ok(bytes)
}

pub fn prune_plan(root: &Path, rels: &[PathBuf], debug: bool) -> io::Result<()> {
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(bad("game path is not a real directory"));
    }
    let files = prune_files(root, rels, debug)?;
    let bytes = prune_measure(&files)?;
    println!("PRUNE|plan|{}|{bytes}", files.len());
    Ok(())
}

pub fn prune_apply(root: &Path, rels: &[PathBuf], debug: bool) -> io::Result<()> {
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(bad("game path is not a real directory"));
    }
    let files = prune_files(root, rels, debug)?;
    let (mut count, mut bytes) = (0u64, 0u64);
    for path in files {
        super::cancelled()?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| bad("prune file escaped game directory"))?
            .to_path_buf();
        let backup = root.join(BACKUP).join(&relative);
        if backup.exists() {
            continue;
        }
        ensure_backup_dirs(root, relative.parent().unwrap_or(Path::new("")))?;
        clone_file(&path, &backup)?;
        let result = (|| -> io::Result<()> {
            let check = sidecar(&backup);
            let mut c = OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(NOFOLLOW)
                .open(&check)?;
            c.write_all(REMOVED)?;
            c.sync_all()?;
            File::open(check.parent().ok_or_else(|| bad("invalid checksum path"))?)?.sync_all()?;
            fs::remove_file(&path)?;
            File::open(path.parent().ok_or_else(|| bad("invalid prune path"))?)?.sync_all()?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&backup);
            let _ = fs::remove_file(sidecar(&backup));
            return Err(e);
        }
        if std::env::var_os("BGC_VERBOSE").is_some() {
            eprintln!("Pruned unused file (restorable): {}", path.display());
        }
        count += 1;
        bytes += fs::metadata(&backup)?.len();
    }
    println!("PRUNE|apply|{count}|{bytes}");
    Ok(())
}

pub fn run(action: &str, target: &str, level: u8, root: &Path) -> io::Result<()> {
    run_internal(action, target, level, root, true)
}

pub fn plan_inventory(target: &str, root: &Path) -> io::Result<()> {
    run_internal("plan", target, 0, root, false)
}

pub fn run_inventory(target: &str, root: &Path) -> io::Result<()> {
    run_internal("plan", target, 0, root, false)
}

pub fn plan_candidates(target: &str, root: &Path) -> io::Result<()> {
    run_internal("plan", target, 0, root, true)
}

fn run_internal(action: &str, target: &str, level: u8, root: &Path, prepare_candidates: bool) -> io::Result<()> {
    let retain_backup = action != "apply-no-backup";
    let action = if retain_backup { action } else { "apply" };
    if !matches!(action, "plan" | "apply" | "restore" | "finalize") {
        return Err(bad("asset action must be plan, apply, restore, or finalize"));
    }
    if !fs::symlink_metadata(root)?.file_type().is_dir() {
        return Err(bad("game path is not a real directory"));
    }
    if action == "restore" || action == "finalize" {
        let backups = walk_backups(root)?;
        for b in &backups {
            let original = original_path(root, b)?;
            if !known_version(&original, b)? {
                return Err(bad(
                    "an asset changed since optimization; refusing restore or backup deletion",
                ));
            }
        }
        return finish_backups(action, root, backups);
    }
    let prof = profile(target)?;
    let Some(p) = prof else {
        println!("ASSETS|{action}|native|0|0|0|0");
        return Ok(());
    };
    if action == "apply" && !(1..=15).contains(&level) {
        return Err(bad("ZSTD level must be 1..15"));
    }
    if !retain_backup {
        eprintln!("Applying assets without persistent restore copies. Runtime compatibility is unverified; recover original assets with Steam verification.");
    }
    let tree = if action == "apply" {
        Some(super::Tree::new(root)?)
    } else {
        None
    };
    let mut count = 0u64;
    let mut before = 0u64;
    let mut after = 0u64;
    let mut retained = 0u64;
    let mut image_count = 0u64;
    let mut texture_count = 0u64;
    let mut audio_count = 0u64;
    let mut package_count = 0u64;
    // Consume each encoded asset immediately instead of retaining an entire
    // game's decoded outputs in RAM. Preview uses the same bounded-memory path.
    let inventory = collect(root, &p, prepare_candidates && (action == "plan" || action == "apply"), |path, bytes, kind| {
        super::cancelled()?;
        let rel = path
            .strip_prefix(root)
            .map_err(|_| bad("asset escaped game directory"))?;
        let backup = root.join(BACKUP).join(rel);
        // A transformed image has an original retained. Never report it as a
        // fresh candidate: applying a second target would currently be
        // ambiguous because the safe source is the backup, not the live file.
        if backup.exists() {
            return Ok(());
        }
        if action == "plan" {
            count += 1;
            match kind {
                AssetKind::Image => image_count += 1,
                AssetKind::Texture => texture_count += 1,
                AssetKind::Audio => audio_count += 1,
                AssetKind::Package => package_count += 1,
            }
            before += fs::metadata(&path)?.len();
            after += bytes.len() as u64;
            return Ok(());
        }
        if action != "apply" {
            return Err(bad(
                "asset action must be plan, apply, restore, or finalize",
            ));
        }
        let parent = rel.parent().unwrap_or(Path::new(""));
        let original_len = fs::metadata(&path)?.len();
        if retain_backup {
            ensure_backup_dirs(root, parent)?;
            clone_file(&path, &backup)?;
        }
        let tmp = append_suffix(&path, &format!(".bgc-tmp-{}", std::process::id()));
        let mut tmp_created = false;
        let mut check_created = false;
        let mut installed = false;
        let write_result = (|| -> io::Result<()> {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(NOFOLLOW)
                .open(&tmp)?;
            tmp_created = true;
            f.write_all(&bytes)?;
            f.sync_all()?;
            let m = fs::metadata(&path)?;
            f.set_permissions(m.permissions())?;
            f.set_times(FileTimes::new().set_modified(m.modified()?))?;
            let rel_tmp = tmp
                .strip_prefix(root)
                .map_err(|_| bad("temporary image escaped game directory"))?;
            let writable = tree
                .as_ref()
                .ok_or_else(|| bad("Btrfs tree was not opened"))?
                .open(rel_tmp, true)?;
            super::compress_file(&writable, rel_tmp, level)?;
            // No-backup mode still uses a new, synced file and atomic rename.
            // The original remains live until the replacement is fully written.
            if retain_backup {
                let check = sidecar(&backup);
                let mut c = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .custom_flags(NOFOLLOW)
                    .open(&check)?;
                check_created = true;
                write!(c, "{:016x}\n", hash(&bytes))?;
                c.sync_all()?;
                File::open(check.parent().ok_or_else(|| bad("invalid checksum path"))?)?.sync_all()?;
            }
            fs::rename(&tmp, &path)?;
            installed = true;
            File::open(path.parent().ok_or_else(|| bad("invalid image path"))?)?.sync_all()?;
            Ok(())
        })();
        if let Err(e) = write_result {
            if tmp_created && !installed {
                let _ = fs::remove_file(&tmp);
            }
            if !installed {
                if check_created {
                    let _ = fs::remove_file(sidecar(&backup));
                }
                if retain_backup { let _ = fs::remove_file(&backup); }
            }
            return Err(e);
        }
        count += 1;
        match kind {
            AssetKind::Image => image_count += 1,
            AssetKind::Texture => texture_count += 1,
            AssetKind::Audio => audio_count += 1,
            AssetKind::Package => package_count += 1,
        }
        let original_len = if retain_backup { fs::metadata(&backup)?.len() } else { original_len };
        before += original_len;
        after += bytes.len() as u64;
        if retain_backup { retained += original_len; }
        if std::env::var_os("BGC_VERBOSE").is_some() {
            let kind_name = match kind {
                AssetKind::Image => "image",
                AssetKind::Texture => "DDS texture",
                AssetKind::Audio => "WAV / standalone FMOD audio",
                AssetKind::Package => "lossless Hades LZ4 package",
            };
            eprintln!(
                "Asset optimized ({kind_name}): {} — {} → {} bytes, {} logical bytes smaller; {}.",
                path.display(), original_len, bytes.len(), original_len.saturating_sub(bytes.len() as u64),
                if retain_backup { "original retained for restore" } else { "no restore copy retained; Steam verification is required for recovery" }
            );
        }
        Ok(())
    })?;
    // Packed PCK assets are handled by a dedicated in-place pass. They are far
    // too large to buffer as a Vec<u8>, and the Godot transforms rewrite the
    // whole container rather than one stream, so they cannot ride the streaming
    // image/audio callback above.
    if action == "apply" || (action == "plan" && prepare_candidates) {
        let packed = packed_asset_pass(root, target, &p, action, level, retain_backup)?;
        count += packed.count;
        before += packed.before;
        after += packed.after;
        retained += packed.retained;
        texture_count += packed.textures;
        audio_count += packed.audio;
    }
    println!(
        "ASSETS|{action}|{}|{count}|{before}|{after}|{retained}|{}|{}|{image_count}|{texture_count}|{audio_count}|{}|{}|{}|{}|{package_count}",
        p.label, inventory.packed_media, inventory.audio_files, inventory.raster_files,
        inventory.logical_bytes, inventory.packed_bytes, inventory.audio_bytes,
    );
    Ok(())
}

#[derive(Default)]
struct PackedStats {
    count: u64,
    before: u64,
    after: u64,
    retained: u64,
    textures: u64,
    audio: u64,
}

/// Walk for Godot PCK files and apply the profile in place.
///
/// Normal apply stages a candidate independently and retains original packs for
/// restore. Explicit apply-no-backup remains irreversible. Unsupported packs
/// stay unchanged and never acquire a backup.
fn packed_asset_pass(root: &Path, target: &str, p: &Profile, action: &str, level: u8, retain_backup: bool) -> io::Result<PackedStats> {
    let mut stats = PackedStats::default();
    if p.max_edge == u32::MAX {
        // Lossless never resizes textures.
        return Ok(stats);
    }
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        super::cancelled()?;
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == BACKUP) {
                continue;
            }
            let m = fs::symlink_metadata(&path)?;
            if m.file_type().is_dir() {
                dirs.push(path);
                continue;
            }
            if !m.file_type().is_file() || m.len() < 52 {
                continue;
            }
            if !path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("pck"))
            {
                continue;
            }
            let original_len = m.len();
            if action == "plan" {
                // A packed pack is a real candidate, but its reduction can only
                // be known after the in-place transform. Count it and report no
                // predicted change rather than overstating the preview.
                stats.count += 1;
                stats.before += original_len;
                stats.after += original_len;
                continue;
            }
            if retain_backup {
                let rel = path.strip_prefix(root).map_err(|_| bad("packed asset escaped root"))?;
                let backup = root.join(BACKUP).join(rel);
                // An existing backup means this pack was already optimized, or
                // needs manual recovery from a previously interrupted operation.
                if backup.exists() || sidecar(&backup).exists() { continue; }
            }
            let result = if retain_backup {
                packed_apply_with_backup(root, &path, target, level)
            } else {
                packed_apply_one(&path, target, level)
            };
            match result {
                Ok((textures, audio)) => {
                    if textures + audio > 0 {
                        let after_len = fs::metadata(&path)?.len();
                        stats.count += 1;
                        stats.before += original_len;
                        stats.after += after_len;
                        if retain_backup { stats.retained += original_len; }
                        stats.textures += textures;
                        stats.audio += audio;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
                Err(e) => {
                    if retain_backup {
                        let rel = path.strip_prefix(root).map_err(|_| bad("packed asset escaped root"))?;
                        if root.join(BACKUP).join(rel).exists() {
                            // Commit or directory-sync may have failed after
                            // publishing the new file. Retained recovery state
                            // makes this a failure, never a silent safe skip.
                            return Err(io::Error::new(e.kind(), format!(
                                "packed optimization needs recovery review: {e}"
                            )));
                        }
                    }
                    if std::env::var_os("BGC_VERBOSE").is_some() {
                        eprintln!("Skipping packed asset {}: {e}", path.display());
                    }
                }
            }
        }
    }
    Ok(stats)
}

/// Prepare an optimized PCK on a separate inode and record a restorable original
/// before publishing it. The checksum sidecar is durable before the commit, so
/// interruption after rename leaves a recognizable backup for normal restore.
/// This is one-file crash recovery, not yet a whole-game transaction.
fn packed_apply_with_backup(root: &Path, path: &Path, target: &str, level: u8) -> io::Result<(u64, u64)> {
    let rel = path.strip_prefix(root).map_err(|_| bad("packed asset escaped root"))?;
    let backup = root.join(BACKUP).join(rel);
    let check = sidecar(&backup);
    if backup.exists() || check.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "packed asset recovery data already exists"));
    }
    let staged = append_suffix(path, &format!(".bgc-packed-stage-{}", std::process::id()));
    let mut backup_created = false;
    let mut committed = false;
    let outcome = (|| -> io::Result<(u64, u64)> {
        clone_file(path, &staged)?;
        let counts = packed_apply_one(&staged, target, level)?;
        if counts.0 + counts.1 == 0 || fs::metadata(&staged)?.len() >= fs::metadata(path)?.len() {
            return Ok((0, 0));
        }
        ensure_backup_dirs(root, rel.parent().unwrap_or(Path::new("")))?;
        clone_file(path, &backup)?;
        backup_created = true;
        if !same_contents(path, &backup)? {
            return Err(bad("source pack changed while preparing a backup"));
        }
        let digest = checksum_file(&staged)?;
        let mut side = OpenOptions::new().write(true).create_new(true).custom_flags(NOFOLLOW).open(&check)?;
        writeln!(side, "{digest:016x}")?;
        side.sync_all()?;
        File::open(check.parent().ok_or_else(|| bad("missing backup parent"))?)?.sync_all()?;
        // Refuse known source changes before the atomic replacement. This does
        // not claim to prevent all hostile concurrent pathname substitutions.
        if !same_contents(path, &backup)? {
            return Err(bad("source pack changed before publication"));
        }
        fs::rename(&staged, path)?;
        committed = true;
        File::open(path.parent().ok_or_else(|| bad("missing game parent"))?)?.sync_all()?;
        Ok(counts)
    })();
    let _ = fs::remove_file(&staged);
    if outcome.is_err() && backup_created && !committed {
        // Original still occupies the game path; only our newly created
        // recovery data may be removed. Never discard backups after commit.
        let _ = fs::remove_file(&check);
        let _ = fs::remove_file(&backup);
    }
    outcome
}

/// Apply the profile to one standalone PCK, returning (textures, audio) counts.
fn packed_apply_one(path: &Path, target: &str, level: u8) -> io::Result<(u64, u64)> {
    // Read the version so Godot 3 and Godot 4 packs route to their own
    // transform. Anything unrecognized stays untouched.
    let mut f = open_read(path)?;
    let _magic = {
        let mut b = [0u8; 4];
        f.read_exact(&mut b)?;
        u32::from_le_bytes(b)
    };
    let mut vb = [0u8; 4];
    f.read_exact(&mut vb)?;
    let version = u32::from_le_bytes(vb);
    drop(f);
    let before = fs::metadata(path)?.len();
    let counts = match version {
        1 => super::containers::godot3_apply(path, 0.0, target, level)?,
        3 | 4 => (super::containers::godot_texture_apply(path, 0.0, target, level)?, 0),
        _ => return Ok((0, 0)),
    };
    // Defence in depth for the savings gate: a transform only installs a
    // temporary when it actually shrank, so a non-shrinking pack keeps its
    // original byte-for-byte content and reports nothing.
    let after = fs::metadata(path)?.len();
    if after >= before {
        return Ok((0, 0));
    }
    Ok(counts)
}

fn finish_backups(action: &str, root: &Path, backups: Vec<PathBuf>) -> io::Result<()> {
    let mut count = 0u64;
    let mut bytes = 0u64;
    for b in backups {
        super::cancelled()?;
        let original = original_path(root, &b)?;
        let side = sidecar(&b);
        if action == "restore" && !same_contents(&original, &b)? {
            let tmp = append_suffix(&original, &format!(".bgc-restore-{}", std::process::id()));
            clone_file(&b, &tmp)?;
            fs::rename(&tmp, &original)?;
            File::open(
                original
                    .parent()
                    .ok_or_else(|| bad("invalid restore path"))?,
            )?
            .sync_all()?;
        } else if action != "restore" && action != "finalize" {
            return Err(bad("invalid asset backup action"));
        }
        bytes += fs::metadata(&b)?.len();
        fs::remove_file(&b)?;
        let _ = fs::remove_file(side);
        count += 1;
    }
    let base = root.join(BACKUP);
    if base.exists() {
        remove_empty_backup_dirs(&base)?;
    }
    println!("ASSETS|{action}|native|{count}|0|0|{bytes}");
    Ok(())
}

// Never recursively delete the restore tree: it may contain unrelated files,
// stale temporary state or symlinks that are not part of this operation.
fn remove_empty_backup_dirs(base: &Path) -> io::Result<()> {
    let mut dirs = vec![base.to_path_buf()];
    let mut index = 0;
    while index < dirs.len() {
        for entry in fs::read_dir(&dirs[index])? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                dirs.push(entry.path());
            }
        }
        index += 1;
    }
    for dir in dirs.into_iter().rev() {
        match fs::remove_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pck_report_reads_versioned_directory_offsets_and_bounds_counts() {
        for version in 1..=4 {
            let mut bytes = vec![0u8; 256];
            let offset = match version { 1 => 84, 2 => 96, _ => 192 };
            // Flags and file-base are not the directory pointer.
            bytes[20..32].fill(255);
            bytes[32..40].copy_from_slice(&(192u64).to_le_bytes());
            bytes[offset..offset + 4].copy_from_slice(&17u32.to_le_bytes());
            let mut file = std::io::Cursor::new(bytes);
            let actual = super::pck_directory_offset(&mut file, version).unwrap();
            assert_eq!(actual, offset as u64);
            assert_eq!(super::pck_directory_count(&mut file, 256, actual).unwrap(), 17);
            assert!(super::pck_directory_count(&mut file, 256, 253).is_err());
            assert!(super::pck_directory_count(&mut file, 256, u64::MAX).is_err());
        }
        assert!(super::pck_directory_offset(&mut std::io::Cursor::new(vec![0; 39]), 3).is_err());
        assert!(super::pck_directory_offset(&mut std::io::Cursor::new(vec![]), 5).is_err());
    }
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(label: &str) -> PathBuf {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("bgc-{label}-{stamp}"));
        fs::create_dir_all(root.join(BACKUP)).unwrap();
        root
    }

    #[test]
    fn balanced_profile_caps_supported_loose_assets_at_1920_pixels() {
        let root = fixture("balanced-1080p-real-image");
        let path = root.join("wallpaper.png");
        let image = image::ImageBuffer::from_fn(3840, 2160, |x, y| {
            image::Rgb([
                ((x.wrapping_mul(13) ^ y.wrapping_mul(7)) & 0xf8) as u8,
                ((x.wrapping_mul(3) + y.wrapping_mul(17)) & 0xf8) as u8,
                ((x.wrapping_mul(19) ^ y.wrapping_mul(23)) & 0xf8) as u8,
            ])
        });
        image.save(&path).unwrap();
        let target = profile("balanced").unwrap().unwrap();
        let encoded = prepare(&path, &target).unwrap().expect("large loose image should produce a smaller candidate");
        let decoded = ImageReader::new(Cursor::new(encoded))
            .with_guessed_format().unwrap().decode().unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1920, 1080));
        assert!(fs::metadata(&path).unwrap().len() > 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_action_is_rejected_even_for_native_or_empty_games() {
        let root = fixture("invalid-action");
        assert!(run("typo", "native", 0, &root).is_err());
        assert!(run("typo", "balanced", 0, &root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn standalone_fmod_uses_profile_policy_in_main_collection() {
        let original = crate::fmod::tests::fixture(false);
        let unsupported = crate::fmod::tests::fixture(true);
        let mut changed = 0;
        for name in ["native", "lossless", "ultra-performance", "performance", "balanced", "quality", "ultra-quality"] {
            let root = fixture("fmod-pipeline");
            let bank = root.join("sound.FSB");
            fs::write(&bank, &original).unwrap();
            fs::write(root.join("unknown.bank"), &unsupported).unwrap();
            fs::write(root.join("resources.resource"), &original).unwrap();
            fs::write(root.join("broken.bank"), b"not a bank").unwrap();
            let mut found = Vec::new();
            if let Some(p) = profile(name).unwrap() {
                let expected = crate::fmod::prepare(&bank, p.audio_target).unwrap();
                collect(&root, &p, true, |path, bytes, kind| {
                    assert!(matches!(kind, AssetKind::Audio));
                    found.push((path, bytes));
                    Ok(())
                }).unwrap();
                if let Some(bytes) = expected {
                    changed += 1;
                    assert_eq!(found, vec![(bank.clone(), bytes.clone())], "{name}");
                    // Main collection never mutates a source, including planning.
                    assert_eq!(fs::read(&bank).unwrap(), original);
                    // Once installed by the main writer, the original backup
                    // prevents cumulative lossy transcoding on subsequent runs.
                    fs::write(root.join(BACKUP).join("sound.FSB"), &original).unwrap();
                    fs::write(&bank, bytes).unwrap();
                    collect(&root, &p, true, |_, _, _| panic!("{name}: backed-up audio was transcoded again")).unwrap();
                } else {
                    assert!(found.is_empty(), "{name}");
                }
            }
            assert_eq!(fs::read(root.join("resources.resource")).unwrap(), original);
            assert_eq!(fs::read(root.join("unknown.bank")).unwrap(), unsupported);
            fs::remove_dir_all(root).unwrap();
        }
        assert!(changed > 0, "the quality guards rejected every fixture; no successful integration exercised");
    }

    #[test]
    fn standalone_fmod_real_btrfs_apply_and_restore() {
        let Some(parent) = std::env::var_os("BGC_TEST_BTRFS_DIR") else { return; };
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = Path::new(&parent).join(format!("bgc-fmod-main-{stamp}"));
        fs::create_dir(&root).unwrap();
        let original = crate::fmod::tests::fixture(false);
        let sound = root.join("sound.fsb");
        fs::write(&sound, &original).unwrap();
        let bank = root.join("sound.bank");
        let mut riff = b"RIFF".to_vec();
        riff.extend_from_slice(&(12u32 + original.len() as u32 + (original.len() as u32 & 1)).to_le_bytes());
        riff.extend_from_slice(b"FEV SND ");
        riff.extend_from_slice(&(original.len() as u32).to_le_bytes());
        riff.extend_from_slice(&original);
        if original.len() & 1 != 0 { riff.push(0); }
        fs::write(&bank, &riff).unwrap();
        let originals = [(sound, original), (bank, riff)];
        // Choose a savings- and waveform-gated tier that actually transforms
        // this synthetic sample, rather than counting a skip as success.
        let name = ["ultra-performance", "performance", "balanced"].into_iter().find(|name| {
            crate::fmod::prepare(&originals[0].0, crate::audio_policy::target_for(name).unwrap()).unwrap().is_some()
        }).expect("no successful FMOD fixture");
        for preserved in ["native", "lossless"] {
            run("apply", preserved, 3, &root).unwrap();
            for (path, bytes) in &originals { assert_eq!(&fs::read(path).unwrap(), bytes); }
        }
        run("plan", name, 3, &root).unwrap();
        for (path, bytes) in &originals { assert_eq!(&fs::read(path).unwrap(), bytes); }
        run("apply", name, 3, &root).unwrap();
        for (path, bytes) in &originals {
            let optimized = fs::read(path).unwrap();
            assert!(optimized.len() < bytes.len());
            let backup = root.join(BACKUP).join(path.file_name().unwrap());
            assert_eq!(&fs::read(&backup).unwrap(), bytes);
            assert!(sidecar(&backup).exists());
        }
        let applied: Vec<_> = originals.iter().map(|(p, _)| fs::read(p).unwrap()).collect();
        run("apply", name, 3, &root).unwrap();
        for ((path, _), bytes) in originals.iter().zip(applied) { assert_eq!(fs::read(path).unwrap(), bytes); }
        run("restore", "native", 3, &root).unwrap();
        for (path, bytes) in &originals { assert_eq!(&fs::read(path).unwrap(), bytes); }
        assert!(!root.join(BACKUP).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prune_backs_up_debug_symbols_marks_removal_and_is_idempotent() {
        let root = fixture("prune-debug");
        fs::write(root.join("game.pdb"), b"symbols").unwrap();
        fs::write(root.join("keep.txt"), b"data").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/engine.ilk"), b"incremental").unwrap();
        prune_apply(&root, &[], true).unwrap();
        assert!(!root.join("game.pdb").exists());
        assert!(!root.join("sub/engine.ilk").exists());
        assert_eq!(fs::read(root.join("keep.txt")).unwrap(), b"data");
        let backup = root.join(BACKUP).join("game.pdb");
        assert_eq!(fs::read(&backup).unwrap(), b"symbols");
        assert_eq!(fs::read(sidecar(&backup)).unwrap(), REMOVED);
        // A removed file is a known state; a resurrected or changed one is not.
        assert!(known_version(&root.join("game.pdb"), &backup).unwrap());
        fs::write(root.join("game.pdb"), b"user restored").unwrap();
        assert!(!known_version(&root.join("game.pdb"), &backup).unwrap());
        // A second pass selects nothing and reports success.
        prune_apply(&root, &[], true).unwrap();
        fs::remove_file(root.join("game.pdb")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prune_rejects_escaping_symlinked_and_missing_targets() {
        use std::os::unix::fs::symlink;
        let root = fixture("prune-safety");
        fs::write(root.join("outside"), b"x").unwrap();
        symlink(root.join("outside"), root.join("link.pdb")).unwrap();
        for rel in ["../outside", "/etc", ".", "missing", ""] {
            assert!(prune_plan(&root, &[PathBuf::from(rel)], false).is_err(), "{rel:?}");
        }
        assert!(prune_plan(&root, &[PathBuf::from("link.pdb")], false).is_err());
        fs::create_dir(root.join("d")).unwrap();
        symlink(root.join("outside"), root.join("d/inner")).unwrap();
        assert!(prune_plan(&root, &[PathBuf::from("d")], false).is_err());
        // The debug scan must not follow symlinks either.
        assert_eq!(prune_files(&root, &[], true).unwrap().len(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prune_plan_reports_selected_regular_files_only() {
        let root = fixture("prune-plan");
        fs::create_dir_all(root.join("suite/deep")).unwrap();
        fs::write(root.join("suite/a.bin"), vec![1u8; 100]).unwrap();
        fs::write(root.join("suite/deep/b.bin"), vec![2u8; 50]).unwrap();
        fs::write(root.join("suite/skip.txt"), b"unused here").unwrap();
        let selected = prune_files(&root, &[PathBuf::from("suite")], false).unwrap();
        assert_eq!(selected.len(), 3);
        assert_eq!(prune_measure(&selected).unwrap(), 100 + 50 + 11);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selective_restore_preserves_other_backups_and_rejects_changed_assets() {
        let root = fixture("selective-restore");
        for name in ["first.pkg","second.pkg"] {
            fs::write(root.join(name),b"candidate").unwrap();
            let backup = root.join(BACKUP).join(name);
            fs::write(&backup,b"original").unwrap();
            fs::write(sidecar(&backup),format!("{:016x}\n",hash(b"candidate"))).unwrap();
        }
        for invalid in ["../first.pkg","/first.pkg","."] {
            assert!(restore_file(&root,Path::new(invalid)).is_err());
        }
        // The unit-test temporary filesystem may not support reflinks. Cover
        // the already-original recovery state here; changed-file restoration
        // is exercised by the opt-in real Btrfs integration test.
        fs::write(root.join("first.pkg"),b"original").unwrap();
        restore_file(&root,Path::new("first.pkg")).unwrap();
        assert_eq!(fs::read(root.join("first.pkg")).unwrap(),b"original");
        assert_eq!(fs::read(root.join("second.pkg")).unwrap(),b"candidate");
        assert!(root.join(BACKUP).join("second.pkg").exists());
        fs::write(root.join("second.pkg"),b"user change").unwrap();
        assert!(restore_file(&root,Path::new("second.pkg")).is_err());
        assert_eq!(fs::read(root.join("second.pkg")).unwrap(),b"user change");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_cleanup_preserves_unrecognized_files_and_symlinks() {
        use std::os::unix::fs::symlink;
        let root = fixture("cleanup");
        let base = root.join(BACKUP);
        fs::create_dir_all(base.join("empty/nested")).unwrap();
        fs::write(base.join("orphan.bgc-checksum"), b"keep").unwrap();
        fs::write(root.join("outside"), b"untouched").unwrap();
        symlink(root.join("outside"), base.join("link")).unwrap();
        finish_backups("finalize", &root, Vec::new()).unwrap();
        assert!(!base.join("empty").exists());
        assert_eq!(fs::read(base.join("orphan.bgc-checksum")).unwrap(), b"keep");
        assert!(fs::symlink_metadata(base.join("link")).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(root.join("outside")).unwrap(), b"untouched");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packed_apply_rewrites_pack_in_place_without_a_backup_dir() {
        // A Godot 3 PCK with one large GDST texture must shrink in place, and
        // no .bgc-assets-backup tree may be created for it: the recovery path
        // for packed Steam assets is "Verify integrity of game files".
        let root = fixture("packed-apply");
        // 1000x600 exceeds the 640px Ultra Performance cap; the gdst unit test
        // already proves this exact fixture shrinks when downscaled.
        let texture = crate::gdst::tests::fixture(1000, 600);
        let name = "res://art.stex";
        let mut bytes = b"GDPC".to_vec();
        for v in [1u32, 3, 7, 0] { bytes.extend_from_slice(&v.to_le_bytes()); }
        bytes.resize(84, 0);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        let offset = 88 + 4 + name.len() + 1 + 32;
        bytes.extend_from_slice(&(name.len() as u32 + 1).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes()); bytes.push(0);
        bytes.extend_from_slice(&(offset as u64).to_le_bytes());
        bytes.extend_from_slice(&(texture.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&crate::md5::digest(&texture));
        bytes.extend_from_slice(&texture);
        let pack = root.join("game.pck");
        fs::write(&pack, &bytes).unwrap();
        let original = fs::read(&pack).unwrap();

        let (textures, _audio) = packed_apply_one(&pack, "ultra-performance", 1).unwrap();
        assert_eq!(textures, 1, "the texture pass must report a rewrite");
        let rewritten = fs::read(&pack).unwrap();
        assert!(rewritten.len() < original.len());
        assert!(!root.join(BACKUP).join("game.pck").exists());

        // A pack that cannot shrink must be left byte-for-byte identical.
        fs::write(&pack, &rewritten).unwrap();
        let (again, _) = packed_apply_one(&pack, "ultra-performance", 1).unwrap();
        assert_eq!(again, 0);
        assert_eq!(fs::read(&pack).unwrap(), rewritten);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packed_apply_with_backup_restores_original_and_finalizes() {
        let root = fixture("packed-recoverable");
        let texture = crate::gdst::tests::fixture(1000, 600);
        let name = "res://art.stex";
        let mut bytes = b"GDPC".to_vec();
        for v in [1u32, 3, 7, 0] { bytes.extend_from_slice(&v.to_le_bytes()); }
        bytes.resize(84, 0);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        let offset = 88 + 4 + name.len() + 1 + 32;
        bytes.extend_from_slice(&(name.len() as u32 + 1).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes()); bytes.push(0);
        bytes.extend_from_slice(&(offset as u64).to_le_bytes());
        bytes.extend_from_slice(&(texture.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&crate::md5::digest(&texture));
        bytes.extend_from_slice(&texture);
        let pack = root.join("game.pck");
        fs::write(&pack, &bytes).unwrap();
        let backup = root.join(BACKUP).join("game.pck");
        let counts = packed_apply_with_backup(&root, &pack, "ultra-performance", 1).unwrap();
        assert_eq!(counts.0, 1);
        assert!(fs::metadata(&pack).unwrap().len() < bytes.len() as u64);
        assert_eq!(fs::read(&backup).unwrap(), bytes);
        assert!(known_version(&pack, &backup).unwrap());
        finish_backups("restore", &root, vec![backup.clone()]).unwrap();
        assert_eq!(fs::read(&pack).unwrap(), bytes);
        assert!(!backup.exists());
        packed_apply_with_backup(&root, &pack, "ultra-performance", 1).unwrap();
        finish_backups("finalize", &root, vec![backup.clone()]).unwrap();
        assert!(!backup.exists());
        assert!(fs::metadata(&pack).unwrap().len() < bytes.len() as u64);
        let no_gain = packed_apply_with_backup(&root, &pack, "ultra-performance", 1).unwrap();
        assert_eq!(no_gain, (0, 0));
        assert!(!backup.exists(), "no-gain pack must not retain a backup");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restore_and_finalize_reject_symlinked_game_subdirectories() {
        use std::os::unix::fs::symlink;
        let root = fixture("restore-link");
        fs::create_dir(root.join("outside")).unwrap();
        fs::create_dir(root.join(BACKUP).join("textures")).unwrap();
        fs::write(root.join("outside/image.png"), b"original").unwrap();
        fs::write(root.join(BACKUP).join("textures/image.png"), b"original").unwrap();
        symlink(root.join("outside"), root.join("textures")).unwrap();
        assert!(run("restore", "native", 0, &root).is_err());
        assert!(run("finalize", "native", 0, &root).is_err());
        assert!(root.join(BACKUP).join("textures/image.png").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_dds_dimensions_are_skipped_without_overflow() {
        let root = fixture("dds-overflow");
        let path = root.join("huge.dds");
        let mut dds = vec![0u8; 128];
        dds[..4].copy_from_slice(b"DDS ");
        dds[4..8].copy_from_slice(&124u32.to_le_bytes());
        dds[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        dds[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        dds[80..84].copy_from_slice(&4u32.to_le_bytes());
        dds[84..88].copy_from_slice(b"DXT5");
        fs::write(&path, dds).unwrap();
        assert!(dds_header(&path).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn streamed_checksum_matches_existing_sidecar_format() {
        let root = fixture("checksum");
        let path = root.join("asset");
        let side = root.join("expected");
        let bytes = (0..200_003).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        fs::write(&path, &bytes).unwrap();
        fs::write(&side, format!("{:016x}\n", hash(&bytes))).unwrap();
        assert!(matches_expected(&path, &side).unwrap());
        fs::write(&path, b"changed").unwrap();
        assert!(!matches_expected(&path, &side).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hades_containers_are_inventoried_not_reported_as_optimized() {
        let root = fixture("hades-inventory");
        for (name, size) in [("animation.bik", 101), ("textures.pkg", 103),
            ("texture.xnb", 107), ("music.bank", 109), ("voice.fsb", 113),
            ("notes.txt", 127)] {
            fs::write(root.join(name), vec![0u8; size]).unwrap();
        }
        fs::write(root.join(BACKUP).join("old.pkg"), vec![0u8; 1000]).unwrap();
        let p = profile("ultra-performance").unwrap().unwrap();
        let inventory = collect(&root, &p, true, |_, _, _| {
            panic!("packed Hades files must not be converted")
        }).unwrap();
        assert_eq!(inventory.packed_media, 3);
        assert_eq!(inventory.packed_bytes, 311);
        assert_eq!(inventory.audio_files, 2);
        assert_eq!(inventory.audio_bytes, 222);
        assert_eq!(inventory.raster_files, 0);
        assert_eq!(inventory.logical_bytes, 660);
        let fast = collect(&root, &p, false, |_, _, _| panic!("inventory mode must not decode candidates")).unwrap();
        assert_eq!(fast.logical_bytes, inventory.logical_bytes);
        let profile = profile("balanced").unwrap().unwrap();
        let candidate_scan = collect(&root, &profile, true, |_, _, _| Ok(())).unwrap();
        assert_eq!(candidate_scan.logical_bytes, inventory.logical_bytes);
        assert!(is_unity_tree_container(Path::new("sharedassets0.resource")));
        assert!(!is_unity_tree_container(Path::new("cover.png")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resizes_qoi_detected_by_signature_without_an_extension() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bgc-assets-unit-{stamp}"));
        fs::create_dir(&root).unwrap();
        let path = root.join("background");
        let image = image::ImageBuffer::from_fn(2048, 256, |x, y| {
            let v = x.wrapping_mul(73).wrapping_add(y.wrapping_mul(151));
            image::Rgb([
                (v & 255) as u8,
                ((v >> 3) & 255) as u8,
                ((v >> 7) & 255) as u8,
            ])
        });
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut encoded, ImageFormat::Qoi)
            .unwrap();
        fs::write(&path, encoded.into_inner()).unwrap();

        let resized = prepare(&path, &profile("ultra-performance").unwrap().unwrap())
            .unwrap()
            .expect("extensionless QOI should be recognized and reduced");
        let decoded = ImageReader::new(Cursor::new(resized))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(decoded.width(), 1024);
        for pixel in decoded.to_rgb8().pixels() {
            assert_eq!(quantize_channel(pixel[0], 7), pixel[0]);
            assert_eq!(quantize_channel(pixel[1], 7), pixel[1]);
            assert_eq!(quantize_channel(pixel[2], 7), pixel[2]);
        }
        let p = profile("ultra-performance").unwrap().unwrap();
        let mut visits = 0;
        collect(&root, &p, true, |candidate, bytes, kind| {
            assert_eq!(candidate, path);
            assert!(!bytes.is_empty());
            assert!(matches!(kind, AssetKind::Image));
            visits += 1;
            Ok(())
        }).unwrap();
        assert_eq!(visits, 1);
        let err = collect(&root, &p, true, |_, _, _| {
            Err(io::Error::new(io::ErrorKind::Interrupted, "stop"))
        }).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packed_resource_extensions_are_not_guessed_as_raster_files() {
        assert!(!may_have_raster_signature(Path::new("texture.tex")));
        assert!(!may_have_raster_signature(Path::new("game.assets")));
        assert!(may_have_raster_signature(Path::new("background")));
    }

    #[test]
    fn loose_atlas_and_thin_images_are_preserved_for_all_profiles() {
        let root = fixture("texture-protection");
        let atlas = root.join("ui_atlas.png");
        let thin = root.join("strip.png");
        image::RgbImage::new(1000, 1000).save(&atlas).unwrap();
        image::RgbImage::new(2048, 65).save(&thin).unwrap();
        for target in ["ultra-performance", "performance", "balanced", "quality", "ultra-quality"] {
            let p = profile(target).unwrap().unwrap();
            assert!(prepare(&atlas, &p).unwrap().is_none(), "{target}");
            assert!(prepare(&thin, &p).unwrap().is_none(), "{target}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_resamples_only_simple_pcm_wav_and_keeps_wave_container() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bgc-wav-unit-{stamp}"));
        fs::create_dir(&root).unwrap();
        let path = root.join("tone.wav");
        let rate = 48_000u32;
        let frames = rate as usize;
        let mut pcm = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let x = (i as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin() * 20_000.0;
            pcm.extend_from_slice(&(x as i16).to_le_bytes());
        }
        let size = 36 + pcm.len() as u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&size.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(&pcm);
        fs::write(&path, wav).unwrap();
        let p = profile("performance").unwrap().unwrap();
        let out = resample_wav(&path, &p).unwrap().unwrap();
        assert_eq!(&out[..4], b"RIFF");
        assert_eq!(&out[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(out[24..28].try_into().unwrap()), 32_000);
        assert!(out.len() < fs::metadata(&path).unwrap().len() as usize);
        let ultra = profile("ultra-performance").unwrap().unwrap();
        let ultra_out = resample_wav(&path, &ultra).unwrap().unwrap();
        assert_eq!(u32::from_le_bytes(ultra_out[24..28].try_into().unwrap()), 11_025);
        assert_eq!(u16::from_le_bytes(ultra_out[32..34].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(ultra_out[34..36].try_into().unwrap()), 8);
        assert!(ultra_out.len() < out.len());
        assert_eq!(ultra_out, resample_wav(&path, &ultra).unwrap().unwrap(), "dither must be deterministic");
        let lossless = profile("lossless").unwrap().unwrap();
        assert!(resample_wav(&path, &lossless).unwrap().is_none());
        let mut float_pcm = Vec::new();
        for i in 0..rate as usize {
            let sample = (i as f64 * 440.0 * std::f64::consts::TAU / rate as f64).sin() as f32;
            float_pcm.extend_from_slice(&sample.to_le_bytes());
        }
        let mut float_wav = Vec::new();
        float_wav.extend_from_slice(b"RIFF"); float_wav.extend_from_slice(&(36u32 + float_pcm.len() as u32).to_le_bytes());
        float_wav.extend_from_slice(b"WAVEfmt "); float_wav.extend_from_slice(&16u32.to_le_bytes());
        float_wav.extend_from_slice(&3u16.to_le_bytes()); float_wav.extend_from_slice(&1u16.to_le_bytes());
        float_wav.extend_from_slice(&rate.to_le_bytes()); float_wav.extend_from_slice(&(rate * 4).to_le_bytes());
        float_wav.extend_from_slice(&4u16.to_le_bytes()); float_wav.extend_from_slice(&32u16.to_le_bytes());
        float_wav.extend_from_slice(b"data"); float_wav.extend_from_slice(&(float_pcm.len() as u32).to_le_bytes());
        float_wav.extend_from_slice(&float_pcm); fs::write(&path, float_wav).unwrap();
        let float_out = resample_wav(&path, &p).unwrap().unwrap();
        assert_eq!(u16::from_le_bytes(float_out[20..22].try_into().unwrap()), 1, "lossy profile emits standard PCM WAV");
        assert_eq!(u16::from_le_bytes(float_out[34..36].try_into().unwrap()), 16);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn visual_profile_quantization_is_monotonic_and_preserves_alpha() {
        let max_error = |bits| (0u16..=255).map(|v| (i16::from(v as u8) - i16::from(quantize_channel(v as u8, bits))).unsigned_abs()).max().unwrap();
        assert!(max_error(5) >= max_error(6));
        assert!(max_error(6) >= max_error(7));
        assert_eq!(max_error(8), 0);
        assert_eq!(quantize_channel(0, 5), 0);
        assert_eq!(quantize_channel(255, 5), 255);
        let mut image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(1, 1, image::Rgba([113, 57, 221, 73])));
        reduce_rgb_precision(&mut image, 5);
        let pixel = image.as_rgba8().unwrap().get_pixel(0, 0).0;
        assert_eq!(pixel[3], 73, "alpha must remain exact");
        assert_ne!(&pixel[..3], &[113, 57, 221]);
        reduce_rgb_precision(&mut image, 8);
        assert_eq!(image.as_rgba8().unwrap().get_pixel(0, 0).0, pixel);
    }

    #[test]
    fn opus_container_is_never_treated_as_wav_or_raster() {
        assert_eq!(format_for(Path::new("music.opus")), None);
    }

    #[test]
    fn bc1_dds_rewrite_preserves_dds_container_and_codec() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bgc-dds-unit-{stamp}"));
        fs::create_dir(&root).unwrap();
        let path = root.join("texture.dds");
        let mut dds = vec![0u8; 128 + 8];
        dds[..4].copy_from_slice(b"DDS ");
        dds[4..8].copy_from_slice(&124u32.to_le_bytes());
        dds[12..16].copy_from_slice(&4u32.to_le_bytes());
        dds[16..20].copy_from_slice(&4u32.to_le_bytes());
        dds[20..24].copy_from_slice(&8u32.to_le_bytes());
        dds[76..80].copy_from_slice(&32u32.to_le_bytes());
        dds[80..84].copy_from_slice(&4u32.to_le_bytes());
        dds[84..88].copy_from_slice(b"DXT1");
        dds[108..112].copy_from_slice(&0x1000u32.to_le_bytes());
        fs::write(&path, dds).unwrap();
        assert!(dds_header(&path).unwrap().is_some());
        let mut alpha_dds = fs::read(&path).unwrap();
        alpha_dds[132..136].copy_from_slice(&3u32.to_le_bytes());
        fs::write(&path, alpha_dds).unwrap();
        assert!(dds_header(&path).unwrap().is_none());
        fs::write(&path, vec![0u8; 136]).unwrap();
        let mut safe_dds = fs::read(&path).unwrap();
        safe_dds[..4].copy_from_slice(b"DDS ");
        safe_dds[4..8].copy_from_slice(&124u32.to_le_bytes());
        safe_dds[12..16].copy_from_slice(&4u32.to_le_bytes());
        safe_dds[16..20].copy_from_slice(&4u32.to_le_bytes());
        safe_dds[20..24].copy_from_slice(&8u32.to_le_bytes());
        safe_dds[76..80].copy_from_slice(&32u32.to_le_bytes());
        safe_dds[80..84].copy_from_slice(&4u32.to_le_bytes());
        safe_dds[84..88].copy_from_slice(b"DXT1");
        safe_dds[108..112].copy_from_slice(&0x1000u32.to_le_bytes());
        fs::write(&path, safe_dds).unwrap();
        let image = DynamicImage::ImageRgba8(image::ImageBuffer::from_fn(4, 4, |x, y| {
            image::Rgba([(x * 61) as u8, (y * 61) as u8, 120, 255])
        }));
        let encoded = encode_dds(&image, &path).unwrap();
        assert_eq!(&encoded[..4], b"DDS ");
        assert_eq!(&encoded[84..88], b"DXT1");
        assert_eq!(u32::from_le_bytes(encoded[16..20].try_into().unwrap()), 4);
        assert_eq!(encoded.len(), 136);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dds_with_mipmaps_is_skipped_instead_of_dropping_levels() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("bgc-dds-mip-unit-{stamp}"));
        fs::create_dir(&root).unwrap();
        let path = root.join("texture.dds");
        let mut dds = vec![0u8; 128 + 8];
        dds[..4].copy_from_slice(b"DDS ");
        dds[4..8].copy_from_slice(&124u32.to_le_bytes());
        dds[12..16].copy_from_slice(&4u32.to_le_bytes());
        dds[16..20].copy_from_slice(&4u32.to_le_bytes());
        dds[20..24].copy_from_slice(&8u32.to_le_bytes());
        dds[28..32].copy_from_slice(&2u32.to_le_bytes());
        dds[76..80].copy_from_slice(&32u32.to_le_bytes());
        dds[80..84].copy_from_slice(&4u32.to_le_bytes());
        dds[84..88].copy_from_slice(b"DXT1");
        dds[108..112].copy_from_slice(&0x1000u32.to_le_bytes());
        fs::write(&path, dds).unwrap();
        assert!(dds_header(&path).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn uncompressed_and_multi_mip_dds_are_now_pipeline_candidates() {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("bgc-dds-rich-unit-{stamp}"));
        fs::create_dir(&root).unwrap();
        let path = root.join("sheet.dds");
        let (w, h) = (1024u32, 1024u32);
        let mut dds = vec![0u8; 128 + (w * h * 4) as usize];
        dds[..4].copy_from_slice(b"DDS ");
        dds[4..8].copy_from_slice(&124u32.to_le_bytes());
        dds[12..16].copy_from_slice(&h.to_le_bytes());
        dds[16..20].copy_from_slice(&w.to_le_bytes());
        dds[20..24].copy_from_slice(&(w * 4).to_le_bytes());
        dds[28..32].copy_from_slice(&1u32.to_le_bytes());
        dds[76..80].copy_from_slice(&32u32.to_le_bytes());
        dds[80..84].copy_from_slice(&0x41u32.to_le_bytes());
        dds[88..92].copy_from_slice(&32u32.to_le_bytes());
        dds[92..96].copy_from_slice(&0x00ff0000u32.to_le_bytes());
        dds[96..100].copy_from_slice(&0x0000ff00u32.to_le_bytes());
        dds[100..104].copy_from_slice(&0x000000ffu32.to_le_bytes());
        dds[104..108].copy_from_slice(&0xff000000u32.to_le_bytes());
        dds[108..112].copy_from_slice(&0x1000u32.to_le_bytes());
        for (i, pixel) in dds[128..].chunks_mut(4).enumerate() {
            let v = (i as u32).wrapping_mul(2_654_435_761);
            pixel.copy_from_slice(&[(v & 255) as u8, ((v >> 8) & 255) as u8, ((v >> 16) & 255) as u8, 255]);
        }
        fs::write(&path, &dds).unwrap();
        // The legacy helper still rejects it, but the pipeline no longer uses it
        // for DDS, so the texture is now a real candidate.
        assert!(dds_header(&path).unwrap().is_none());
        let p = profile("ultra-performance").unwrap().unwrap();
        let out = prepare(&path, &p).unwrap().expect("uncompressed DDS should be a candidate");
        assert!(out.len() < dds.len());
        assert_eq!(&out[..4], b"DDS ");
        assert_eq!(fs::read(&path).unwrap(), dds, "planning must not modify the source");
        fs::remove_dir_all(root).unwrap();
    }
}
