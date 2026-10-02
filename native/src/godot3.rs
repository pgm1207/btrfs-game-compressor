//! Godot 3 (`RSRC` binary resources and standalone PCK v1) audio optimization.
//!
//! Godot 3 stores binary resources with the `RSRC` magic and only serializes
//! properties that differ from their defaults. This module can read those
//! resources and rebuild them from scratch, then re-encode packed audio:
//!
//! - `AudioStreamMP3` (`.mp3str`) is decoded and re-encoded as Ogg Vorbis. The
//!   resource type becomes `AudioStreamOGGVorbis`; the matching `.import` stub's
//!   `type=` is patched by the PCK layer so the engine still loads it. This is
//!   lossy and profile-gated.
//!
//! Everything is fail-closed: unknown property variants, channel counts or
//! encodings leave the entry untouched.
use std::{io, num::{NonZeroU32, NonZeroU8}};
use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};

fn bad(s: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}
fn cancelled() -> io::Result<()> { super::cancelled() }

#[derive(Clone, Debug, PartialEq)]
pub enum Variant {
    Nil,
    Bool(bool),
    Int(i32),
    Int64(i64),
    Real(f32),
    Double(f64),
    Str(String),
    Raw(Vec<u8>),
}

pub struct Resource {
    pub res_type: String,
    pub props: Vec<(String, Variant)>,
}
impl Resource {
    pub fn get(&self, name: &str) -> Option<&Variant> {
        self.props.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }
    pub fn raw(&self, name: &str) -> Option<&[u8]> {
        match self.get(name) {
            Some(Variant::Raw(bytes)) => Some(bytes),
            _ => None,
        }
    }
}

fn put_ustring(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&((s.len() + 1) as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
    out.push(0);
}
fn write_variant(out: &mut Vec<u8>, value: &Variant) {
    match value {
        Variant::Nil => out.extend_from_slice(&1u32.to_le_bytes()),
        Variant::Bool(v) => {
            out.extend_from_slice(&2u32.to_le_bytes());
            out.extend_from_slice(&u32::from(*v).to_le_bytes());
        }
        Variant::Int(v) => {
            out.extend_from_slice(&3u32.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        Variant::Real(v) => {
            out.extend_from_slice(&4u32.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        Variant::Str(v) => {
            out.extend_from_slice(&5u32.to_le_bytes());
            put_ustring(out, v);
        }
        Variant::Raw(bytes) => {
            out.extend_from_slice(&31u32.to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
        }
        Variant::Int64(v) => {
            out.extend_from_slice(&40u32.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        Variant::Double(v) => {
            out.extend_from_slice(&41u32.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
}

/// Build a standalone Godot 3 `RSRC` resource containing exactly `props`.
/// Godot only stores non-default properties, so callers decide what to include.
pub fn write_resource(res_type: &str, props: &[(&str, Variant)]) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"RSRC");
    out.extend_from_slice(&0u32.to_le_bytes()); // big endian
    out.extend_from_slice(&0u32.to_le_bytes()); // use real64
    out.extend_from_slice(&3u32.to_le_bytes()); // engine major
    out.extend_from_slice(&5u32.to_le_bytes()); // engine minor
    out.extend_from_slice(&3u32.to_le_bytes()); // format version
    put_ustring(&mut out, res_type);
    out.extend_from_slice(&0u64.to_le_bytes()); // import metadata offset
    for _ in 0..14 {
        out.extend_from_slice(&0u32.to_le_bytes());
    }
    out.extend_from_slice(&(props.len() as u32).to_le_bytes()); // string table
    for (name, _) in props {
        put_ustring(&mut out, name);
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // external resources
    out.extend_from_slice(&1u32.to_le_bytes()); // internal resources
    put_ustring(&mut out, "local://0");
    let offset_field = out.len();
    out.extend_from_slice(&0u64.to_le_bytes()); // body offset placeholder
    let body_offset = out.len() as u64;
    out[offset_field..offset_field + 8].copy_from_slice(&body_offset.to_le_bytes());
    put_ustring(&mut out, res_type);
    out.extend_from_slice(&(props.len() as u32).to_le_bytes());
    for (index, (_, value)) in props.iter().enumerate() {
        out.extend_from_slice(&(index as u32).to_le_bytes());
        write_variant(&mut out, value);
    }
    Ok(out)
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| bad("resource overflow"))?;
        if end > self.b.len() {
            return Err(bad("truncated resource"));
        }
        let out = &self.b[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn ustring(&mut self) -> io::Result<String> {
        let len = self.u32()? as usize;
        if len == 0 || len > 65535 {
            return Err(bad("invalid string length"));
        }
        let bytes = self.take(len)?;
        let trimmed = bytes.strip_suffix(&[0]).unwrap_or(bytes);
        String::from_utf8(trimmed.to_vec()).map_err(|_| bad("invalid UTF-8 string"))
    }
    fn string_or_index(&mut self, strings: &[String]) -> io::Result<String> {
        let id = self.u32()?;
        if id & 0x8000_0000 != 0 {
            let len = (id & 0x7FFF_FFFF) as usize;
            if len == 0 || len > 65535 {
                return Err(bad("invalid inline string"));
            }
            let bytes = self.take(len)?;
            let trimmed = bytes.strip_suffix(&[0]).unwrap_or(bytes);
            return String::from_utf8(trimmed.to_vec()).map_err(|_| bad("invalid inline UTF-8"));
        }
        strings.get(id as usize).cloned().ok_or_else(|| bad("property name index out of range"))
    }
    fn variant(&mut self) -> io::Result<Variant> {
        let kind = self.u32()?;
        Ok(match kind {
            1 => Variant::Nil,
            2 => Variant::Bool(self.u32()? != 0),
            3 => Variant::Int(self.u32()? as i32),
            4 => Variant::Real(f32::from_bits(self.u32()?)),
            5 => Variant::Str(self.ustring()?),
            31 => {
                let len = self.u32()? as usize;
                if len > 512 * 1024 * 1024 {
                    return Err(bad("oversized resource array"));
                }
                Variant::Raw(self.take(len)?.to_vec())
            }
            40 => Variant::Int64(i64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            41 => Variant::Double(f64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            _ => return Err(bad("unsupported resource property type")),
        })
    }
}

/// Read a standalone Godot 3 `RSRC` resource into its type and properties.
pub fn parse(bytes: &[u8]) -> io::Result<Resource> {
    if bytes.len() < 4 || &bytes[..4] != b"RSRC" {
        return Err(bad("not a Godot 3 RSRC resource"));
    }
    let mut r = Reader::new(bytes);
    r.take(4)?;
    let _big_endian = r.u32()?;
    let _real64 = r.u32()?;
    let _major = r.u32()?;
    let _minor = r.u32()?;
    let _format = r.u32()?;
    let res_type = r.ustring()?;
    let _import_md = r.u64()?;
    for _ in 0..14 {
        let _ = r.u32()?;
    }
    let string_count = r.u32()? as usize;
    if string_count > 65535 {
        return Err(bad("resource string table too large"));
    }
    let mut strings = Vec::with_capacity(string_count);
    for _ in 0..string_count {
        strings.push(r.ustring()?);
    }
    let external = r.u32()? as usize;
    if external > 4096 {
        return Err(bad("resource external table too large"));
    }
    for _ in 0..external {
        let _ = r.ustring()?;
        let _ = r.ustring()?;
    }
    let internal = r.u32()? as usize;
    if internal != 1 {
        return Err(bad("unsupported multi-resource file"));
    }
    let _path = r.ustring()?;
    let body_offset = r.u64()? as usize;
    if body_offset >= bytes.len() {
        return Err(bad("resource body outside file"));
    }
    r.pos = body_offset;
    let _body_type = r.ustring()?;
    let property_count = r.u32()? as usize;
    if property_count > 4096 {
        return Err(bad("resource property count too large"));
    }
    let mut props = Vec::with_capacity(property_count);
    for _ in 0..property_count {
        let name = r.string_or_index(&strings)?;
        let value = r.variant()?;
        props.push((name, value));
    }
    Ok(Resource { res_type, props })
}

fn pump(decoded: &nanomp3::Decoded<f32>, channels: usize, quality: f32) -> io::Result<Vec<u8>> {
    if decoded.sample_rate == 0 || channels == 0 || channels > 2 {
        return Err(bad("unsupported decoded audio"));
    }
    let mut builder = VorbisEncoderBuilder::new_with_serial(
        NonZeroU32::new(decoded.sample_rate).ok_or_else(|| bad("bad sample rate"))?,
        NonZeroU8::new(channels as u8).ok_or_else(|| bad("bad channel count"))?,
        Vec::new(),
        1,
    );
    builder.bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr { target_quality: quality });
    let mut encoder = builder.build().map_err(|e| bad(&format!("vorbis encoder: {e:?}")))?;
    let frames = decoded.samples.len() / channels;
    let mut planar: Vec<Vec<f32>> = (0..channels).map(|_| Vec::with_capacity(4096)).collect();
    let mut done = 0;
    while done < frames {
        cancelled()?;
        let count = (frames - done).min(4096);
        for (channel, plane) in planar.iter_mut().enumerate() {
            plane.clear();
            for frame in done..done + count {
                plane.push(decoded.samples[frame * channels + channel]);
            }
        }
        encoder.encode_audio_block(&planar).map_err(|e| bad(&format!("vorbis encode: {e:?}")))?;
        done += count;
    }
    encoder.finish().map_err(|e| bad(&format!("vorbis finish: {e:?}")))
}

/// Re-encode an `AudioStreamMP3` resource as an `AudioStreamOGGVorbis` resource.
/// The stored bytes stay under the same entry name; only the resource type (and
/// the matching `.import` stub) changes. Returns `Ok(None)` when the entry is not
/// a supported MP3 or is not worth rewriting.
pub fn transform_mp3(bytes: &[u8], quality: f32) -> io::Result<Option<Vec<u8>>> {
    let resource = match parse(bytes) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    if resource.res_type != "AudioStreamMP3" {
        return Ok(None);
    }
    let data = match resource.raw("data") {
        Some(d) if !d.is_empty() => d,
        _ => return Ok(None),
    };
    let decoded = nanomp3::decode_all::<f32>(data);
    if decoded.samples.is_empty() || decoded.format_changed {
        return Ok(None);
    }
    let channels = match decoded.channels {
        Some(c) => c.num() as usize,
        None => return Ok(None),
    };
    let ogg = pump(&decoded, channels, quality)?;
    let loop_enabled = matches!(resource.get("loop"), Some(Variant::Bool(true)));
    let loop_offset = match resource.get("loop_offset") {
        Some(Variant::Real(v)) => *v,
        _ => 0.0,
    };
    let mut props: Vec<(&str, Variant)> = vec![("data", Variant::Raw(ogg))];
    if loop_enabled {
        props.push(("loop", Variant::Bool(true)));
        if loop_offset != 0.0 {
            props.push(("loop_offset", Variant::Real(loop_offset)));
        }
    }
    let out = write_resource("AudioStreamOGGVorbis", &props)?;
    if out.len() >= bytes.len() {
        return Ok(None);
    }
    Ok(Some(out))
}

/// Cheap read-only description: resource type and the size of its `data` array.
#[allow(dead_code)]
pub fn describe(bytes: &[u8]) -> Option<(String, u64)> {
    let resource = parse(bytes).ok()?;
    let data = resource.raw("data").map(|d| d.len() as u64).unwrap_or(0);
    Some((resource.res_type, data))
}

// ---------------------------------------------------------------------------
// IMA-ADPCM (Godot 3 `AudioStreamSample` format 2)
//
// Godot 3's own playback decoder starts predictor/index at zero and reads
// headerless nibbles, low first. Stereo interleaves packed channel bytes.
// See scene/resources/audio_stream_sample.cpp in Godot's 3.x branch.
// ---------------------------------------------------------------------------
const IMA_STEP: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50,
    55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279,
    307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282,
    1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871,
    5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818,
    18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const IMA_INDEX: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// Re-encode an `AudioStreamSample` resource's PCM as IMA-ADPCM (format 2).
/// Only plain 16-bit PCM mono/stereo is converted; everything else is skipped.
/// Research only: disabled in the exporter after a real Brotato menu crash.
#[allow(dead_code)]
pub fn transform_sample(bytes: &[u8]) -> io::Result<Option<Vec<u8>>> {
    let resource = match parse(bytes) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    if resource.res_type != "AudioStreamSample" {
        return Ok(None);
    }
    // 1 = 16-bit PCM. 8-bit and already-compressed formats are left alone.
    if !matches!(resource.get("format"), Some(Variant::Int(1))) {
        return Ok(None);
    }
    let data = match resource.raw("data") {
        Some(d) if d.len() >= 2 && d.len() % 2 == 0 => d,
        _ => return Ok(None),
    };
    let stereo = matches!(resource.get("stereo"), Some(Variant::Bool(true)));
    let channels = if stereo { 2 } else { 1 };
    let frames = data.len() / 2 / channels;
    // The engine derives duration from payload length, with two frames per
    // packed byte. Leave odd lengths untouched rather than add a frame.
    if frames == 0 || frames % 2 != 0 || data.len() % (2 * channels) != 0 {
        return Ok(None);
    }
    // IMA playback forces every loop to forward; preserve other loop modes.
    if !matches!(resource.get("loop_mode"), None | Some(Variant::Int(0 | 1))) {
        return Ok(None);
    }
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    if stereo {
        // Godot's PCM `data` is interleaved: L,R,L,R,...
        for f in 0..frames {
            left.push(i16::from_le_bytes([data[f * 4], data[f * 4 + 1]]));
            right.push(i16::from_le_bytes([data[f * 4 + 2], data[f * 4 + 3]]));
        }
    } else {
        for f in 0..frames {
            left.push(i16::from_le_bytes([data[f * 2], data[f * 2 + 1]]));
        }
    }
    // Godot reads channel `c` sample `i` from
    //   byte = payload[(i >> 1) * channels + c], nibble = (i & 1) ? hi : lo
    // so the payload interleaves one packed byte per channel per nibble-pair,
    // There are no headers in the playback payload.
    let mut encoded = Vec::with_capacity(frames / 2 * channels);
    let mut state = vec![(0i32, 0i32); channels];
    for pair in 0..(frames + 1) / 2 {
        // Emit one byte per channel for this nibble pair.
        for c in 0..channels {
            let mut byte = 0u8;
            for half in 0..2u32 {
                let f = pair * 2 + half as usize;
                if f >= frames { break; }
                let sample = if c == 0 { left[f] } else { right[f] };
                let (mut predictor, mut step_index) = state[c];
                let step = IMA_STEP[step_index as usize];
                let mut diff = sample as i32 - predictor;
                let mut nibble = 0u8;
                if diff < 0 { nibble = 8; diff = -diff; }
                let mut vpdiff = step >> 3;
                if diff >= step { nibble |= 4; diff -= step; vpdiff += step; }
                if diff >= step >> 1 { nibble |= 2; diff -= step >> 1; vpdiff += step >> 1; }
                if diff >= step >> 2 { nibble |= 1; vpdiff += step >> 2; }
                predictor += if nibble & 8 != 0 { -vpdiff } else { vpdiff };
                predictor = predictor.clamp(-32768, 32767);
                step_index = (step_index + IMA_INDEX[nibble as usize] as i32).clamp(0, 88);
                state[c] = (predictor, step_index);
                if half == 0 { byte = nibble; } else { byte |= nibble << 4; }
            }
            encoded.push(byte);
        }
    }
    if encoded.len() >= data.len() {
        return Ok(None);
    }
    // Do not trade heavily transient sounds for noisy ADPCM. This is a
    // conservative engineering gate, not a guarantee of perceptual quality.
    for c in 0..channels {
        let original = if c == 0 { &left } else { &right };
        let decoded = ima_decode(&encoded, frames, channels, c);
        let signal: f64 = original.iter().map(|&v| (v as f64).powi(2)).sum();
        let noise: f64 = original.iter().zip(&decoded)
            .map(|(&a, &b)| (a as f64 - b as f64).powi(2)).sum();
        if decoded.len() != frames || (noise > 0.0 && signal < noise * 100.0) {
            return Ok(None);
        }
    }
    // Godot 3 serializes only non-default properties, so mirror the source's
    // own set and swap `data`/`format`.
    let mut props: Vec<(&str, Variant)> = Vec::new();
    for (name, value) in &resource.props {
        match name.as_str() {
            "data" => props.push(("data", Variant::Raw(encoded.clone()))),
            "format" => props.push(("format", Variant::Int(2))),
            _ => props.push((name.as_str(), value.clone())),
        }
    }
    if !resource.props.iter().any(|(n, _)| n == "data") {
        props.insert(0, ("data", Variant::Raw(encoded)));
    }
    if !resource.props.iter().any(|(n, _)| n == "format") {
        props.push(("format", Variant::Int(2)));
    }
    let out = write_resource("AudioStreamSample", &props)?;
    if out.len() >= bytes.len() {
        return Ok(None);
    }
    Ok(Some(out))
}

/// Rewrite only property values in a single-resource file, preserving its
/// original header, internal path, string table, property identifiers and tail.
fn patch_properties(bytes: &[u8], changes: &[(String, Variant)]) -> io::Result<Vec<u8>> {
    let mut r = Reader::new(bytes);
    r.take(24)?;
    r.ustring()?;
    if r.u64()? != 0 { return Err(bad("import metadata offsets unsupported")); }
    r.take(56)?;
    let count = r.u32()?;
    let strings: Vec<String> = (0..count).map(|_| r.ustring()).collect::<io::Result<_>>()?;
    if r.u32()? != 0 || r.u32()? != 1 { return Err(bad("complex sample resource")); }
    r.ustring()?;
    r.pos = r.u64()? as usize;
    r.ustring()?;
    let count_pos = r.pos;
    let count = r.u32()?;
    let mut out = bytes[..r.pos].to_vec();
    let mut seen = Vec::new();
    for _ in 0..count {
        let start = r.pos;
        let name = r.string_or_index(&strings)?;
        let value_start = r.pos;
        r.variant()?;
        if let Some((_, value)) = changes.iter().find(|(n, _)| n == &name) {
            out.extend_from_slice(&bytes[start..value_start]);
            write_variant(&mut out, value);
            seen.push(name);
        } else {
            out.extend_from_slice(&bytes[start..r.pos]);
        }
    }
    let mut added = 0;
    for (name, value) in changes {
        if seen.contains(name) { continue; }
        // Godot's inline property name representation includes a trailing NUL.
        out.extend_from_slice(&(0x80000000u32 | (name.len() as u32 + 1)).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        write_variant(&mut out, value);
        added += 1;
    }
    out[count_pos..count_pos + 4].copy_from_slice(&(count + added).to_le_bytes());
    out.extend_from_slice(&bytes[r.pos..]);
    Ok(out)
}

/// Profile-aware PCM reduction; no ADPCM and no resource-type change.
pub fn transform_sample_pcm(bytes: &[u8], profile: &str) -> io::Result<Option<Vec<u8>>> {
    let target = match crate::audio_policy::target_for(profile)? { Some(t) => t, None => return Ok(None) };
    let (limit_rate, limit_bits) = (target.pcm_rate, target.pcm_bits as usize);
    let res = match parse(bytes) { Ok(r) => r, Err(_) => return Ok(None) };
    if res.res_type != "AudioStreamSample" || bytes[4..12] != [0; 8] { return Ok(None); }
    let format = match res.get("format") { None => 0, Some(Variant::Int(v @ (0 | 1))) => *v, _ => return Ok(None) };
    let bits = if format == 0 { 8 } else { 16 };
    let channels = match res.get("stereo") { None | Some(Variant::Bool(false)) => 1, Some(Variant::Bool(true)) => 2, _ => return Ok(None) };
    let rate = match res.get("mix_rate") { None => 44100, Some(Variant::Int(v)) if (8000..=192000).contains(v) => *v as u32, _ => return Ok(None) };
    let out_rate = rate.min(limit_rate);
    let out_bits = bits.min(limit_bits);
    if rate == out_rate && bits == out_bits { return Ok(None); }
    let data = match res.raw("data") { Some(d) => d, None => return Ok(None) };
    let stride = channels * (bits / 8);
    if data.is_empty() || data.len() % stride != 0 { return Ok(None); }
    let frames = data.len() / stride;
    let mut out_frames = ((frames as u64 * out_rate as u64 + rate as u64 / 2) / rate as u64) as usize;
    // Keep byte arrays 4-byte aligned without relying on loader padding rules.
    let frame_alignment = 4 / (channels * (out_bits / 8));
    out_frames -= out_frames % frame_alignment;
    if out_frames == 0 { return Ok(None); }
    let sample = |frame: usize, c: usize| -> f64 {
        let offset = (frame * channels + c) * (bits / 8);
        if bits == 8 { data[offset] as i8 as f64 / 128.0 }
        else { i16::from_le_bytes([data[offset], data[offset + 1]]) as f64 / 32768.0 }
    };
    let ratio = out_rate as f64 / rate as f64;
    let radius = 16.0 / ratio;
    let mut audio = Vec::with_capacity(out_frames * channels * (out_bits / 8));
    for frame in 0..out_frames {
        if frame % 4096 == 0 { cancelled()?; }
        let center = frame as f64 / ratio;
        for c in 0..channels {
            let value = if out_rate == rate { sample(frame, c) } else {
                let (mut sum, mut weights) = (0.0, 0.0);
                for i in (center - radius).floor() as isize..=(center + radius).ceil() as isize {
                    let distance = i as f64 - center;
                    if distance.abs() >= radius { continue; }
                    let x = distance * ratio * std::f64::consts::PI;
                    let sinc = if x.abs() < 1e-9 { 1.0 } else { x.sin() / x };
                    let weight = sinc * (0.5 + 0.5 * (std::f64::consts::PI * distance / radius).cos());
                    sum += sample(i.clamp(0, frames as isize - 1) as usize, c) * weight;
                    weights += weight;
                }
                if weights.abs() < 1e-12 { 0.0 } else { sum / weights }
            };
            if out_bits == 8 {
                // Godot stores signed PCM8, unlike unsigned PCM8 in WAV files.
                audio.push((value.mul_add(128.0, 0.0).round().clamp(-128.0, 127.0) as i8) as u8);
            } else {
                audio.extend_from_slice(&( (value * 32768.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes());
            }
        }
    }
    let mut changes = vec![("data".into(), Variant::Raw(audio)),
        ("format".into(), Variant::Int(if out_bits == 8 { 0 } else { 1 })),
        ("mix_rate".into(), Variant::Int(out_rate as i32))];
    // Loop indices are frames, not bytes; scale both endpoints with the rate.
    for name in ["loop_begin", "loop_end"] {
        if let Some(value) = res.get(name) {
            let Variant::Int(v) = value else { return Ok(None) };
            if *v < 0 || *v as usize > frames { return Ok(None); }
            let scaled = ((*v as u64 * out_rate as u64 + rate as u64 / 2) / rate as u64).min(out_frames as u64);
            changes.push((name.into(), Variant::Int(scaled as i32)));
        }
    }
    if matches!(res.get("loop_mode"), Some(Variant::Int(v)) if *v != 0) {
        let endpoint = |name: &str| changes.iter().find(|(n, _)| n == name).and_then(|(_, v)| if let Variant::Int(i) = v { Some(*i) } else { None });
        if endpoint("loop_begin").unwrap_or(0) >= endpoint("loop_end").unwrap_or(0) { return Ok(None); }
    }
    let out = match patch_properties(bytes, &changes) { Ok(out) => out, Err(_) => return Ok(None) };
    let check = parse(&out)?;
    for (name, value) in &changes {
        if check.get(name) != Some(value) { return Err(bad("PCM rewrite verification failed")); }
    }
    Ok((out.len() < bytes.len()).then_some(out))
}

/// True when a `.import` stub's `type=` should be switched to Ogg Vorbis.
pub fn rewrite_import_type(bytes: &[u8], converted: &[Vec<u8>]) -> Option<Vec<u8>> {
    let old = b"type=\"AudioStreamMP3\"";
    if !bytes.windows(old.len()).any(|w| w == old) {
        return None;
    }
    // Stubs point at the resource by its bare file name inside `.import/`, so
    // match the basename of each converted entry rather than its `res://` path.
    if !converted.iter().any(|name| {
        let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
        let base = name[..end].rsplit(|b| *b == b'/').next().unwrap_or(&name[..end]);
        !base.is_empty() && bytes.windows(base.len()).any(|w| w == base)
    }) {
        return None;
    }
    let text = String::from_utf8(bytes.to_vec()).ok()?;
    let patched = text.replace("type=\"AudioStreamMP3\"", "type=\"AudioStreamOGGVorbis\"");
    if patched.len() == text.len() || !patched.contains("type=\"AudioStreamOGGVorbis\"") {
        return None;
    }
    Some(patched.into_bytes())
}

/// Reference decoder matching Godot's headerless IMA playback layout.
/// `data` contains interleaved packed bytes; `channel` selects
/// which channel to reconstruct.
pub fn ima_decode(data: &[u8], frames: usize, channels: usize, channel: usize) -> Vec<i16> {
    if channels == 0 || channel >= channels || frames == 0 {
        return Vec::new();
    }
    let mut predictor = 0i32;
    let mut step_index = 0i32;
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        let nibble_index = (i >> 1) * channels + channel;
        let pos = nibble_index;
        if pos >= data.len() {
            break;
        }
        let byte = data[pos];
        let nibble = if i & 1 == 0 { byte & 0x0F } else { byte >> 4 };
        let step = IMA_STEP[step_index as usize];
        // Godot's decoder: bit0 = step/4, bit1 = step/2, bit2 = step, bit3 = sign.
        let mut vpdiff = step >> 3;
        if nibble & 1 != 0 { vpdiff += step >> 2; }
        if nibble & 2 != 0 { vpdiff += step >> 1; }
        if nibble & 4 != 0 { vpdiff += step; }
        predictor += if nibble & 8 != 0 { -vpdiff } else { vpdiff };
        predictor = predictor.clamp(-32768, 32767);
        step_index = (step_index + IMA_INDEX[nibble as usize] as i32).clamp(0, 88);
        out.push(predictor as i16);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_profiles_reduce_rate_and_depth_without_changing_resource_class() {
        let frames = 4800usize;
        let mut audio = Vec::new();
        for i in 0..frames {
            let v = ((i as f64 * std::f64::consts::TAU * 440.0 / 48000.0).sin() * 16000.0) as i16;
            audio.extend_from_slice(&v.to_le_bytes());
            audio.extend_from_slice(&(-v).to_le_bytes());
        }
        let input = write_resource("AudioStreamSample", &[
            ("data", Variant::Raw(audio)), ("format", Variant::Int(1)),
            ("mix_rate", Variant::Int(48000)), ("stereo", Variant::Bool(true)),
            ("loop_mode", Variant::Int(2)), ("loop_begin", Variant::Int(480)),
            ("loop_end", Variant::Int(4320)),
        ]).unwrap();
        for (profile, rate, bits) in [("ultra-performance", 11025, 8), ("performance", 32000, 16), ("balanced", 44100, 16)] {
            let output = transform_sample_pcm(&input, profile).unwrap().unwrap();
            let res = parse(&output).unwrap();
            assert_eq!(res.res_type, "AudioStreamSample");
            assert_eq!(res.get("mix_rate"), Some(&Variant::Int(rate)));
            assert_eq!(res.get("format"), Some(&Variant::Int(if bits == 8 { 0 } else { 1 })));
            assert_eq!(res.get("loop_mode"), Some(&Variant::Int(2)));
            let frames = res.raw("data").unwrap().len() / (2 * bits / 8);
            assert!((frames as f64 / rate as f64 - 0.1).abs() < 4.0 / rate as f64);
            assert_eq!(output, transform_sample_pcm(&input, profile).unwrap().unwrap());
        }
        for profile in ["native", "lossless", "quality", "ultra-quality"] {
            assert!(transform_sample_pcm(&input, profile).unwrap().is_none());
        }
    }

    #[test]
    fn pcm8_is_signed_and_missing_rate_is_added_as_inline_property() {
        let input = write_resource("AudioStreamSample", &[
            ("data", Variant::Raw(vec![128; 400])), // signed -128
        ]).unwrap();
        let output = transform_sample_pcm(&input, "ultra-performance").unwrap().unwrap();
        let res = parse(&output).unwrap();
        assert_eq!(res.get("mix_rate"), Some(&Variant::Int(11025)));
        assert!(res.raw("data").unwrap().iter().all(|&v| v == 128));
        assert_eq!(res.get("format"), Some(&Variant::Int(0)));
    }

    #[test]
    fn godot_headerless_known_vector_and_stereo_stride() {
        // Calculated from Godot's zero-initialized decoder: low nibble first.
        assert_eq!(ima_decode(&[0x77, 0x00], 4, 1, 0), vec![11, 41, 45, 48]);
        assert_eq!(ima_decode(&[0x77, 0xff, 0x00, 0x88], 4, 2, 0), vec![11, 41, 45, 48]);
        assert_eq!(ima_decode(&[0x77, 0xff, 0x00, 0x88], 4, 2, 1), vec![-11, -41, -45, -48]);
    }

    #[test]
    fn sample_transform_preserves_duration_and_loop_semantics() {
        for (frames, mode) in [(101usize, 0), (100, 2), (100, 3)] {
            let input = write_resource("AudioStreamSample", &[
                ("data", Variant::Raw(vec![0; frames * 4])),
                ("format", Variant::Int(1)), ("stereo", Variant::Bool(true)),
                ("loop_mode", Variant::Int(mode)),
            ]).unwrap();
            assert!(transform_sample(&input).unwrap().is_none());
        }
        let input = write_resource("AudioStreamSample", &[
            ("data", Variant::Raw(vec![0; 400])), ("format", Variant::Int(1)),
            ("stereo", Variant::Bool(true)), ("loop_mode", Variant::Int(1)),
            ("loop_begin", Variant::Int(10)), ("loop_end", Variant::Int(90)),
            ("mix_rate", Variant::Int(48000)),
        ]).unwrap();
        let output = parse(&transform_sample(&input).unwrap().unwrap()).unwrap();
        assert_eq!(output.raw("data").unwrap().len(), 100);
        for (name, value) in parse(&input).unwrap().props {
            if name != "data" && name != "format" { assert_eq!(output.get(&name), Some(&value)); }
        }
    }

    #[test]
    fn ima_sample_round_trips_within_tolerance() {
        // A 440 Hz tone, the kind of waveform real SFX are made of. IMA-ADPCM is
        // lossy and its error scales with amplitude, so judge it by SNR rather
        // than by a fixed per-sample bound.
        let frames = 4096usize;
        let mut pcm_16 = Vec::with_capacity(frames);
        for i in 0..frames {
            let t = i as f64 / 44100.0;
            let v = (t * 440.0 * std::f64::consts::TAU).sin() * 0.7;
            pcm_16.push((v * 32767.0) as i16);
        }
        let mut stereo_16 = Vec::with_capacity(frames * 4);
        // Interleaved L,R as Godot stores it.
        for f in 0..frames {
            stereo_16.extend_from_slice(&pcm_16[f].to_le_bytes());
            stereo_16.extend_from_slice(&pcm_16[f].to_le_bytes());
        }
        let pcm_len = stereo_16.len();
        let resource = write_resource(
            "AudioStreamSample",
            &[("data", Variant::Raw(stereo_16)), ("format", Variant::Int(1)), ("stereo", Variant::Bool(true))],
        )
        .unwrap();
        let converted = transform_sample(&resource).unwrap().expect("should convert");
        let parsed = parse(&converted).unwrap();
        assert!(matches!(parsed.get("format"), Some(Variant::Int(2))), "format must become IMA-ADPCM");
        assert!(matches!(parsed.get("stereo"), Some(Variant::Bool(true))), "channel layout must be kept");
        let data = parsed.raw("data").unwrap();
        assert!(data.len() < pcm_len, "IMA must be smaller than PCM16");
        assert_eq!(data.len(), frames);
        let left = ima_decode(data, frames, 2, 0);
        let right = ima_decode(data, frames, 2, 1);
        let (mut signal, mut noise) = (0.0f64, 0.0f64);
        for f in 0..frames {
            signal += (pcm_16[f] as f64).powi(2);
            noise += (left[f] as f64 - pcm_16[f] as f64).powi(2);
            // Both channels must reconstruct the same signal.
            assert_eq!(left[f], right[f], "stereo channels must not be crossed");
        }
        let snr = 10.0 * (signal / noise.max(1.0)).log10();
        assert!(snr > 20.0, "IMA SNR {snr:.1} dB is implausibly low");
    }

    #[test]
    fn sample_transform_skips_unsupported_and_already_small() {
        // 8-bit PCM is not converted (Godot serializes it differently).
        let eight = write_resource(
            "AudioStreamSample",
            &[("data", Variant::Raw(vec![0u8; 4096])), ("format", Variant::Int(0))],
        )
        .unwrap();
        assert!(transform_sample(&eight).unwrap().is_none());
        // Already IMA.
        let ima = write_resource(
            "AudioStreamSample",
            &[("data", Variant::Raw(vec![0u8; 64])), ("format", Variant::Int(2))],
        )
        .unwrap();
        assert!(transform_sample(&ima).unwrap().is_none());
        // Not an AudioStreamSample at all.
        assert!(transform_sample(b"garbage").unwrap().is_none());
    }

    #[test]
    fn resource_round_trips_all_supported_variants() {
        let props = vec![
            ("data", Variant::Raw(vec![0, 1, 2, 3, 4])),
            ("format", Variant::Int(2)),
            ("stereo", Variant::Bool(true)),
            ("mix_rate", Variant::Real(22050.0)),
            ("name", Variant::Str("hello".into())),
        ];
        let bytes = write_resource("AudioStreamSample", &props).unwrap();
        let parsed = parse(&bytes).unwrap();
        assert_eq!(parsed.res_type, "AudioStreamSample");
        assert_eq!(parsed.raw("data").unwrap(), &[0, 1, 2, 3, 4]);
        assert!(matches!(parsed.get("format"), Some(Variant::Int(2))));
        assert!(matches!(parsed.get("stereo"), Some(Variant::Bool(true))));
        assert!(matches!(parsed.get("mix_rate"), Some(Variant::Real(v)) if (*v - 22050.0).abs() < 0.5));
        assert!(matches!(parsed.get("name"), Some(Variant::Str(s)) if s == "hello"));
    }

    #[test]
    fn non_audio_and_garbage_are_rejected() {
        assert!(parse(b"not a resource").is_err());
        assert!(transform_mp3(b"garbage", 0.2).unwrap().is_none());
        let text = write_resource("AudioStreamSample", &[("format", Variant::Int(1))]).unwrap();
        assert!(transform_mp3(&text, 0.2).unwrap().is_none());
    }

    #[test]
    fn import_type_rewrite_only_touches_matching_stubs() {
        // Stubs reference the dest by bare file name inside `.import/`.
        let mp3_name = b"song.mp3-deadbeef.mp3str".to_vec();
        let stub = b"[remap]\n\nimporter=\"mp3\"\ntype=\"AudioStreamMP3\"\npath=\"res://.import/song.mp3-deadbeef.mp3str\"\n".to_vec();
        let patched = rewrite_import_type(&stub, &[mp3_name.clone()]).unwrap();
        assert!(String::from_utf8(patched).unwrap().contains("AudioStreamOGGVorbis"));
        assert!(rewrite_import_type(&stub, &[b"other.mp3str".to_vec()]).is_none());
        assert!(rewrite_import_type(b"unrelated text", &[mp3_name]).is_none());
        assert!(rewrite_import_type(&stub, &[b"res://.import/song.mp3-deadbeef.mp3str\0\0\0".to_vec()]).is_some());
    }
}
