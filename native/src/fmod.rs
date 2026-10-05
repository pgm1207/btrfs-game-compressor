//! Experimental native FSB5 Vorbis transcoding for exports and asset apply.
//! Bank/event metadata and playback sample rates are retained. Unity .resource
//! slices are not standalone banks: their serialized references are not rewritten.
use std::{fs, io::{self, Read, Write, Cursor}, num::{NonZeroU32, NonZeroU8},
    os::unix::fs::{MetadataExt, OpenOptionsExt}, path::Path};
use lewton::{header::{read_header_ident, read_header_setup},
    audio::{read_audio_packet_generic, PreviousWindowRight}};
use vorbis_rs::{VorbisEncoderBuilder, VorbisBitrateManagementStrategy};
#[path = "../vendor/fsbex/vorbis_lookup.rs"]
mod vorbis_lookup;

const MAX_FILE: u64 = 1024 * 1024 * 1024;
const RATES: [u32; 10] = [4000, 8000, 11000, 11025, 16000, 22050, 24000, 32000, 44100, 48000];
#[derive(Clone, Copy, Debug)]
struct Policy {
    quality: f32,
    min_snr_db: f64,
    min_gain_pct: usize,
}
impl From<f32> for Policy {
    fn from(quality: f32) -> Self {
        Self { quality, min_snr_db: 15.0, min_gain_pct: 0 }
    }
}
fn policy(name: &str) -> io::Result<Policy> {
    let p = match name {
        "conservative" => Policy { quality: 0.3, min_snr_db: 20.0, min_gain_pct: 5 },
        "balanced" => Policy { quality: 0.2, min_snr_db: 18.0, min_gain_pct: 5 },
        _ => name.parse::<f32>().map(Policy::from)
            .map_err(|_| bad("FMOD profile must be conservative, balanced, or quality 0.0..0.4"))?,
    };
    if !p.quality.is_finite() || !(0.0..=0.4).contains(&p.quality) {
        return Err(bad("FMOD quality must be 0.0..0.4"));
    }
    Ok(p)
}

// Sparse, aligned decoded-PCM comparison bounds memory to ~21 MiB even at
// the maximum stream duration. This catches timing shifts and excessive
// waveform damage; it is a rejection guard, not a perceptual quality promise.
#[derive(Default)]
struct QualityCheck {
    reference: Vec<[f32; 2]>,
    signal: f64,
    error: f64,
    checked: usize,
}
impl QualityCheck {
    fn capture(&mut self, pcm: &[Vec<f32>], start: usize, count: usize) {
        let first = (32 - start % 32) % 32;
        for i in (first..count).step_by(32) {
            self.reference.push([pcm[0][i], pcm.get(1).map_or(0.0, |ch| ch[i])]);
        }
    }
    fn compare(&mut self, pcm: &[Vec<f32>], start: usize, frames: usize) {
        let count = pcm[0].len().min(frames.saturating_sub(start));
        let first = (32 - start % 32) % 32;
        for i in (first..count).step_by(32) {
            let at = (start+i)/32;
            if let Some(reference) = self.reference.get(at) {
                for (ch, samples) in pcm.iter().enumerate() {
                    let original = f64::from(reference[ch]);
                    self.signal += original * original;
                    self.error += (original-f64::from(samples[i])).powi(2);
                    self.checked += 1;
                }
            }
        }
    }
    fn acceptable(&self, min_snr_db: f64) -> bool {
        self.checked > 0 && self.signal.is_finite() && self.error.is_finite()
            && (self.error <= 1e-12 || (self.signal / self.error).log10()*10.0 >= min_snr_db)
    }
}
fn bad(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
fn slice(b: &[u8], p: usize, n: usize) -> io::Result<&[u8]> {
    b.get(p..p.checked_add(n).ok_or_else(|| bad("FMOD offset overflow"))?)
        .ok_or_else(|| bad("truncated FMOD data"))
}
fn u32_at(b: &[u8], p: usize) -> io::Result<u32> {
    Ok(u32::from_le_bytes(slice(b, p, 4)?.try_into().unwrap()))
}
#[derive(Clone, Debug)]
struct Sample {
    word: u64,
    rate: u32,
    channels: u8,
    frames: usize,
    offset: usize,
    extras: Vec<(u8, Vec<u8>)>,
}
impl Sample {
    fn crc(&self) -> Option<u32> {
        self.extras.iter().find(|(kind, _)| *kind == 11)
            .and_then(|(_, data)| data.get(..4))
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
}
#[derive(Debug)]
struct Bank<'a> {
    header: &'a [u8],
    names: &'a [u8],
    audio: &'a [u8],
    samples: Vec<Sample>,
}
fn parse(b: &[u8]) -> io::Result<Bank<'_>> {
    if slice(b,0,4)? != b"FSB5" || u32_at(b,4)? != 1 || u32_at(b,24)? != 15 {
        return Err(bad("only unencrypted FSB5 v1 Vorbis is supported"));
    }
    let count = u32_at(b,8)? as usize;
    if count == 0 || count > 50000 { return Err(bad("invalid FSB sample count")); }
    let headers_end = 60usize.checked_add(u32_at(b,12)? as usize).ok_or_else(|| bad("FSB size overflow"))?;
    let names_size = u32_at(b,16)? as usize;
    let data_start = headers_end.checked_add(names_size).ok_or_else(|| bad("FSB size overflow"))?;
    let data_size = u32_at(b,20)? as usize;
    if data_start.checked_add(data_size) != Some(b.len()) {
        return Err(bad("FSB size fields do not match its container"));
    }
    let mut pos = 60;
    let mut samples = Vec::new();
    for _ in 0..count {
        let word = u64::from_le_bytes(slice(b,pos,8)?.try_into().unwrap()); pos += 8;
        let rate_index = ((word >> 1) & 15) as usize;
        let mut sample = Sample { word, rate: *RATES.get(rate_index).unwrap_or(&0),
            channels: 1 + ((word >> 5) & 1) as u8, frames: (word >> 34) as usize,
            offset: (((word >> 6) & 0x0fff_ffff) * 16) as usize, extras: Vec::new() };
        let mut more = word & 1 != 0;
        while more {
            if pos + 4 > headers_end { return Err(bad("FSB extra header escaped header table")); }
            let extra = u32_at(b,pos)?; pos += 4;
            more = extra & 1 != 0;
            let len = ((extra >> 1) & 0x00ff_ffff) as usize;
            let kind = (extra >> 25) as u8;
            if pos.checked_add(len).is_none_or(|end| end > headers_end) {
                return Err(bad("FSB extra metadata escaped header table"));
            }
            let data = slice(b,pos,len)?.to_vec(); pos += len;
            match kind {
                1 if len == 1 => sample.channels = data[0],
                2 if len == 4 => sample.rate = u32::from_le_bytes(data[..4].try_into().unwrap()),
                _ => {}
            }
            sample.extras.push((kind,data));
        }
        if sample.offset >= data_size || (samples.is_empty() && sample.offset != 0)
            || samples.last().is_some_and(|previous: &Sample| previous.offset >= sample.offset) {
            return Err(bad("invalid FSB sample offsets"));
        }
        samples.push(sample);
    }
    if pos > headers_end || slice(b,pos,headers_end-pos)?.iter().any(|v| *v != 0) {
        return Err(bad("unrecognized FSB header-table padding"));
    }
    let names = slice(b,headers_end,names_size)?;
    if !names.is_empty() {
        if names.len() < count * 4 { return Err(bad("truncated FSB name offsets")); }
        for i in 0..count {
            let offset = u32_at(names,i*4)? as usize;
            if offset < count * 4 || !names.get(offset..).is_some_and(|name| name.contains(&0)) {
                return Err(bad("invalid FSB sample name"));
            }
        }
    }
    Ok(Bank { header: slice(b,0,60)?, names, audio: slice(b,data_start,data_size)?, samples })
}
fn identification(rate: u32, channels: u8) -> Vec<u8> {
    let mut b = vec![1]; b.extend_from_slice(b"vorbis");
    b.extend_from_slice(&0u32.to_le_bytes()); b.push(channels);
    b.extend_from_slice(&rate.to_le_bytes()); b.extend_from_slice(&[0;12]);
    b.extend_from_slice(&[0xb8,1]); b
}
fn encoder(quality: f32, channels: u8) -> io::Result<vorbis_rs::VorbisEncoder<Vec<u8>>> {
    // FSB stores playback rate separately. Using the FMOD-compatible 32kHz
    // codebook family preserves sample counts/rate without resampling. Verify
    // the resulting setup against the bundled FMOD table before accepting it.
    let mut builder = VorbisEncoderBuilder::new_with_serial(NonZeroU32::new(32000).unwrap(),
        NonZeroU8::new(channels).ok_or_else(|| bad("zero channels"))?, Vec::new(), 1);
    builder.bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr { target_quality: quality });
    builder.build().map_err(bad)
}
fn transcode(sample: &Sample, data: &[u8], policy: Policy) -> io::Result<Option<(Vec<u8>,Vec<u8>)>> {
    if !(1..=2).contains(&sample.channels) || sample.rate == 0 || sample.frames == 0
        || sample.frames > 48_000 * 60 * 30 {
        return Ok(None);
    }
    // Unknown metadata can refer to encoded packet offsets. Do not blindly
    // carry such offsets across a codec change. Timing/loop/marker and channel
    // metadata is retained, while type 11 seek data is rebuilt.
    if sample.extras.iter().any(|(kind, _)| !matches!(kind,1|2|3|4|11|13)) { return Ok(None); }
    if sample.extras.iter().filter(|(kind,_)| *kind == 11).count() != 1 { return Ok(None); }
    let Some(setup) = sample.crc().and_then(|crc| vorbis_lookup::VORBIS_LOOKUP.get(&crc)) else {
        return Ok(None);
    };
    for (kind, bytes) in &sample.extras {
        if *kind == 3 && (bytes.len() != 8 || u32_at(bytes,0)? > u32_at(bytes,4)?
            || u32_at(bytes,4)? as usize >= sample.frames) { return Ok(None); }
    }
    let id = read_header_ident(&identification(sample.rate,sample.channels)).map_err(bad)?;
    let setup = read_header_setup(setup,sample.channels,(8,11)).map_err(bad)?;
    let mut window = PreviousWindowRight::new();
    let mut encoder = encoder(policy.quality,sample.channels)?;
    let mut quality_check = QualityCheck::default();
    let (mut pos, mut frames) = (0usize,0usize);
    while pos + 2 <= data.len() {
        super::cancelled()?;
        let len = u16::from_le_bytes(slice(data,pos,2)?.try_into().unwrap()) as usize;
        pos += 2;
        if len == 0 || len == 65535 { break; }
        let packet = slice(data,pos,len)?; pos += len;
        let pcm: Vec<Vec<f32>> = read_audio_packet_generic(&id,&setup,packet,&mut window).map_err(bad)?;
        if pcm.len() != sample.channels as usize { return Err(bad("decoded FSB channel mismatch")); }
        let count = pcm[0].len().min(sample.frames.saturating_sub(frames));
        if count > 0 {
            quality_check.capture(&pcm,frames,count);
            let block: Vec<_> = pcm.iter().map(|channel| &channel[..count]).collect();
            encoder.encode_audio_block(block).map_err(bad)?;
            frames += count;
        }
        if frames == sample.frames { break; }
    }
    if frames != sample.frames { return Err(bad("FSB decoded audio is shorter than declared duration")); }
    let ogg = encoder.finish().map_err(bad)?;
    let mut reader = ogg::reading::PacketReader::new(Cursor::new(ogg));
    let new_id = reader.read_packet().map_err(bad)?.ok_or_else(|| bad("missing Vorbis ID"))?;
    let _comment = reader.read_packet().map_err(bad)?.ok_or_else(|| bad("missing Vorbis comment"))?;
    let new_setup = reader.read_packet().map_err(bad)?.ok_or_else(|| bad("missing Vorbis setup"))?;
    let crc = crc32fast::hash(&new_setup.data);
    if new_id.data.get(28) != Some(&0xb8)
        || vorbis_lookup::VORBIS_LOOKUP.get(&crc).is_none_or(|known| **known != new_setup.data) {
        return Err(bad("encoder produced a setup unsupported by the bundled FMOD codebook table"));
    }
    let new_setup_header = read_header_setup(&new_setup.data,sample.channels,(8,11)).map_err(bad)?;
    let mut check_window = PreviousWindowRight::new();
    let (mut output,mut seek) = (Vec::new(),Vec::new());
    let mut decoded_frames = 0usize;
    while let Some(packet) = reader.read_packet().map_err(bad)? {
        super::cancelled()?;
        let n = packet.data.len();
        if n == 0 || n >= 65535 { return Err(bad("invalid re-encoded FSB packet size")); }
        let decoded: Vec<Vec<f32>> = read_audio_packet_generic(&id,&new_setup_header,&packet.data,&mut check_window).map_err(bad)?;
        quality_check.compare(&decoded,decoded_frames,sample.frames);
        let offset = u32::try_from(output.len()).map_err(bad)?;
        // Record offsets at packet boundaries using decoded frame positions,
        // not Ogg page granules (which describe page ends, not packet starts).
        if decoded_frames < sample.frames && (seek.is_empty() || decoded_frames / sample.rate as usize
            > u32_at(&seek,seek.len()-8)? as usize / sample.rate as usize) {
            seek.extend_from_slice(&(decoded_frames.min(sample.frames) as u32).to_le_bytes());
            seek.extend_from_slice(&offset.to_le_bytes());
        }
        decoded_frames += decoded[0].len();
        output.extend_from_slice(&(n as u16).to_le_bytes()); output.extend_from_slice(&packet.data);
    }
    if decoded_frames < sample.frames { return Err(bad("re-encoded FSB duration verification failed")); }
    if !quality_check.acceptable(policy.min_snr_db) {
        if std::env::var_os("BGC_VERBOSE").is_some() {
            eprintln!("Retaining FMOD stream: decoded waveform quality/timing guard rejected candidate");
        }
        return Ok(None);
    }
    output.extend_from_slice(&0u16.to_le_bytes());
    output.resize(output.len().div_ceil(32)*32,0);
    let mut extra = crc.to_le_bytes().to_vec();
    extra.extend_from_slice(&(seek.len() as u32).to_le_bytes()); extra.extend(seek);
    Ok(Some((output,extra)))
}
fn rebuild_fsb(b: &[u8], policy: Policy) -> io::Result<(Vec<u8>,usize,usize)> {
    let bank = parse(b)?;
    // Unknown fields might contain absolute bank offsets, not just offsets
    // relative to their own stream. Retaining that stream while relocating it
    // would not be safe, so leave the entire bank unchanged in this case.
    if bank.samples.iter().any(|sample| sample.extras.iter()
        .any(|(kind,_)| !matches!(kind,1|2|3|4|11|13))) {
        return Ok((b.to_vec(),0,bank.samples.len()));
    }
    let mut audio = Vec::new(); let mut headers = Vec::new();
    let (mut changed,mut skipped) = (0usize,0usize);
    for (i, sample) in bank.samples.iter().enumerate() {
        super::cancelled()?;
        if std::env::var_os("BGC_ASSET_PROGRESS").is_some() {
            eprintln!("Experimental FMOD transcode: stream {}/{}",i+1,bank.samples.len());
        }
        let end = bank.samples.get(i+1).map_or(bank.audio.len(),|next| next.offset);
        let original = slice(bank.audio,sample.offset,end-sample.offset)?;
        let mut extras = sample.extras.clone();
        let new = transcode(sample,original,policy)?;
        let old_seek_size = extras.iter().find(|(kind,_)| *kind == 11).map_or(0,|(_,data)| data.len());
        let old_size = original.len()+old_seek_size;
        let selected = if let Some((new,seek)) = new.as_ref().filter(|(new,seek)|
            (new.len()+seek.len())*100 < old_size*(100-policy.min_gain_pct)) {
            extras.iter_mut().find(|(kind,_)| *kind == 11).unwrap().1 = seek.clone();
            changed += 1; &new[..]
        } else { skipped += 1; original };
        audio.resize(audio.len().div_ceil(32)*32,0);
        let units = audio.len()/16;
        if units > 0x0fff_ffff { return Err(bad("FSB data offset exceeds format limits")); }
        let word = (sample.word & !(0x0fff_ffffu64 << 6)) | ((units as u64) << 6);
        headers.extend_from_slice(&word.to_le_bytes());
        for (index,(kind,data)) in extras.iter().enumerate() {
            if data.len() > 0x00ff_ffff { return Err(bad("FSB extra data exceeds format limits")); }
            let flags = (u32::from(*kind) << 25) | ((data.len() as u32) << 1)
                | u32::from(index+1 < extras.len());
            headers.extend_from_slice(&flags.to_le_bytes()); headers.extend_from_slice(data);
        }
        audio.extend_from_slice(selected);
    }
    if changed == 0 { return Ok((b.to_vec(),0,skipped)); }
    let mut names = bank.names.to_vec();
    // Header/name padding is zero-filled. Relative name offsets stay unchanged.
    if names.is_empty() { headers.resize((60+headers.len()).div_ceil(16)*16-60,0); }
    else { names.resize((60+headers.len()+names.len()).div_ceil(16)*16-60-headers.len(),0); }
    let mut output = bank.header.to_vec();
    output[12..16].copy_from_slice(&(headers.len() as u32).to_le_bytes());
    output[16..20].copy_from_slice(&(names.len() as u32).to_le_bytes());
    output[20..24].copy_from_slice(&(audio.len() as u32).to_le_bytes());
    output.extend(headers); output.extend(names); output.extend(audio);
    let rebuilt = parse(&output)?;
    for (old,new) in bank.samples.iter().zip(&rebuilt.samples) {
        if old.rate != new.rate || old.channels != new.channels || old.frames != new.frames
            || old.extras.iter().filter(|(k,_)| *k != 11).collect::<Vec<_>>()
                != new.extras.iter().filter(|(k,_)| *k != 11).collect::<Vec<_>>() {
            return Err(bad("FSB metadata verification failed"));
        }
    }
    Ok((output,changed,skipped))
}
fn rebuild(b: &[u8], policy: Policy) -> io::Result<(Vec<u8>,usize,usize)> {
    if b.starts_with(b"FSB5") { return rebuild_fsb(b,policy); }
    if slice(b,0,4)? != b"RIFF" || slice(b,8,4)? != b"FEV " || u32_at(b,4)? as usize+8 != b.len() {
        return Err(bad("expected an FSB5 file or RIFF/FEV FMOD bank"));
    }
    let mut pos = 12; let mut snd = None;
    while pos < b.len() {
        let size = u32_at(b,pos+4)? as usize;
        let payload = slice(b,pos+8,size)?;
        if slice(b,pos,4)? == b"SND " {
            if snd.is_some() { return Err(bad("multiple FSB sections are not supported")); }
            let prefix = payload.windows(4).position(|v| v == b"FSB5").ok_or_else(|| bad("SND has no FSB5 header"))?;
            if prefix > 32 || payload[..prefix].iter().any(|v| *v != 0) {
                return Err(bad("unrecognized FMOD SND prefix"));
            }
            snd = Some((pos,size,prefix));
        }
        pos += 8+size+(size&1);
    }
    if pos != b.len() { return Err(bad("invalid RIFF padding")); }
    let (pos,size,prefix) = snd.ok_or_else(|| bad("bank has no audio section"))?;
    let (fsb,changed,skipped) = rebuild_fsb(slice(b,pos+8+prefix,size-prefix)?,policy)?;
    let new_size = prefix+fsb.len();
    let mut output = b[..pos+4].to_vec();
    output.extend_from_slice(&(new_size as u32).to_le_bytes());
    output.extend_from_slice(&b[pos+8..pos+8+prefix]); output.extend(fsb);
    if new_size&1 != 0 { output.push(0); }
    output.extend_from_slice(&b[pos+8+size+(size&1)..]);
    let len = output.len() as u32-8; output[4..8].copy_from_slice(&len.to_le_bytes());
    Ok((output,changed,skipped))
}
fn read_source(input: &Path) -> io::Result<Vec<u8>> {
    // Explicit symlink refusal: do not depend on O_NOFOLLOW alone. An aarch64
    // release runner followed a symlink and produced a candidate bundle.
    if fs::symlink_metadata(input)?.file_type().is_symlink() {
        return Err(bad("FMOD input must not be a symlink"));
    }
    let mut source = fs::OpenOptions::new().read(true).custom_flags(0x20000 | 0x800).open(input)?;
    let meta = source.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 || meta.len() > MAX_FILE { return Err(bad("FMOD input must be a single-link regular file of at most 1 GiB")); }
    let mut b = Vec::new(); Read::by_ref(&mut source).take(MAX_FILE+1).read_to_end(&mut b)?;
    let after = source.metadata()?;
    if b.len() as u64 != meta.len() || meta.len() != after.len()
        || meta.mtime() != after.mtime() || meta.mtime_nsec() != after.mtime_nsec()
        || meta.ctime() != after.ctime() || meta.ctime_nsec() != after.ctime_nsec() {
        return Err(bad("FMOD input changed during read"));
    }
    Ok(b)
}

/// Prepare only a standalone bank, never an embedded Unity/Unreal resource.
/// The main asset writer supplies savings-gated replacement, original backup,
/// checksum, compression, and restore. Unknown metadata/codebooks fail closed.
pub fn prepare(input: &Path, target: Option<crate::audio_policy::Target>) -> io::Result<Option<Vec<u8>>> {
    let Some(target) = target else { return Ok(None); };
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !ext.eq_ignore_ascii_case("fsb") && !ext.eq_ignore_ascii_case("bank") { return Ok(None); }
    let bytes = read_source(input)?;
    let policy = Policy { quality: target.vorbis_quality, min_snr_db: target.min_snr_db, min_gain_pct: 5 };
    let (result, changed, _) = rebuild(&bytes, policy)?;
    if changed == 0 || result.len() >= bytes.len() { return Ok(None); }
    Ok(Some(result))
}

/// Bounded offline decoder microbenchmark, not an FMOD runtime benchmark.
/// Up to eight evenly spaced streams, five seconds each; five warm repetitions.
pub fn audit(input: &Path) -> io::Result<()> {
    let bytes = read_source(input)?;
    let fsb = if bytes.starts_with(b"FSB5") { &bytes[..] } else {
        if slice(&bytes,0,4)? != b"RIFF" || slice(&bytes,8,4)? != b"FEV "
            || u32_at(&bytes,4)? as usize+8 != bytes.len() {
            return Err(bad("expected FSB5 or RIFF/FEV"));
        }
        let mut pos = 12;
        let mut section = None;
        while pos < bytes.len() {
            let size = u32_at(&bytes,pos+4)? as usize;
            let payload = slice(&bytes,pos+8,size)?;
            if slice(&bytes,pos,4)? == b"SND " {
                if section.is_some() { return Err(bad("multiple audio sections")); }
                let prefix = payload.windows(4).position(|p| p == b"FSB5")
                    .ok_or_else(|| bad("no FSB5 in SND"))?;
                if prefix > 32 || payload[..prefix].iter().any(|v| *v != 0) {
                    return Err(bad("unknown SND prefix"));
                }
                section = Some(&payload[prefix..]);
            }
            pos += 8+size+(size&1);
        }
        if pos != bytes.len() { return Err(bad("invalid RIFF padding")); }
        section.ok_or_else(|| bad("no audio section"))?
    };
    let bank = parse(fsb)?;
    let stride = bank.samples.len().div_ceil(8);
    let mut streams = 0;
    let mut frames = 0;
    let mut seconds = 0.0f64;
    let mut best_total = 0.0;
    for index in (0..bank.samples.len()).step_by(stride) {
        let sample = &bank.samples[index];
        if sample.rate == 0 || !(1..=2).contains(&sample.channels) { continue; }
        let Some(setup) = sample.crc().and_then(|crc| vorbis_lookup::VORBIS_LOOKUP.get(&crc)) else { continue; };
        let id = read_header_ident(&identification(sample.rate,sample.channels)).map_err(bad)?;
        let setup = read_header_setup(setup,sample.channels,(8,11)).map_err(bad)?;
        let end = bank.samples.get(index+1).map_or(bank.audio.len(),|next| next.offset);
        let data = &bank.audio[sample.offset..end];
        let limit = sample.frames.min(sample.rate as usize*5);
        if limit == 0 { continue; }
        let mut best = f64::INFINITY;
        let mut verified_frames = 0;
        for _ in 0..5 {
            let mut window = PreviousWindowRight::new();
            let mut count = 0;
            let mut pos = 0;
            let mut elapsed = std::time::Duration::ZERO;
            while count < limit && pos+2 <= data.len() {
                super::cancelled()?;
                let n = u16::from_le_bytes(slice(data,pos,2)?.try_into().unwrap()) as usize;
                pos += 2;
                if n == 0 || n == 65535 { break; }
                let packet = slice(data,pos,n)?; pos += n;
                let started = std::time::Instant::now();
                let pcm: Vec<Vec<f32>> = read_audio_packet_generic(&id,&setup,packet,&mut window).map_err(bad)?;
                elapsed += started.elapsed();
                if pcm.len() != sample.channels as usize || pcm.iter().flatten().any(|v| !v.is_finite()) {
                    return Err(bad("invalid decoded audio"));
                }
                count += pcm[0].len();
            }
            if count < limit { return Err(bad("sample is shorter than declared duration")); }
            verified_frames = count;
            best = best.min(elapsed.as_secs_f64());
        }
        streams += 1;
        frames += verified_frames;
        seconds += verified_frames as f64 / sample.rate as f64;
        best_total += best;
    }
    if streams == 0 { return Err(bad("no supported streams to benchmark")); }
    eprintln!("Sampled offline lewton decoder benchmark; excludes I/O and setup. Not in-game FMOD CPU or seek validation.");
    println!("FMOD_AUDIT|{streams}|{frames}|{seconds:.6}|{best_total:.6}|5");
    Ok(())
}

pub fn run(profile: &str, input: &Path, output: &Path) -> io::Result<()> {
    let policy = policy(profile)?;
    let b = read_source(input)?;
    eprintln!("Experimental lossy FMOD export; source stays unchanged. In-game playback/loop testing is required before replacement.");
    let (result,changed,skipped) = rebuild(&b,policy)?;
    if changed == 0 || result.len() >= b.len() {
        println!("FMOD|{}|{}|0|{skipped}|NO_OUTPUT",b.len(),b.len()); return Ok(());
    }
    super::cancelled()?;
    let mut target = fs::OpenOptions::new().write(true).create_new(true).custom_flags(0x20000).open(output)?;
    let write = (|| {
        for chunk in result.chunks(65536) { super::cancelled()?; target.write_all(chunk)?; }
        super::cancelled()?;
        target.sync_all()
    })();
    if let Err(error) = write { let _ = fs::remove_file(output); return Err(error); }
    println!("FMOD|{}|{}|{changed}|{skipped}|EXPORTED",b.len(),result.len());
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn fixture(extra_unknown: bool) -> Vec<u8> {
        let frames = 44100usize;
        let mut enc = encoder(0.4,2).unwrap();
        let mut seed = 0x12345678u32;
        let pcm: Vec<f32> = (0..frames).map(|i| {
            seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
            let t = i as f32 / 44100.0;
            0.4*(t*440.0*std::f32::consts::TAU).sin()
                + 0.2*(t*2137.0*std::f32::consts::TAU).sin()
                + 0.1*(seed as f32 / u32::MAX as f32 * 2.0 - 1.0)
        }).collect();
        for chunk in pcm.chunks(4096) { enc.encode_audio_block([chunk,chunk]).unwrap(); }
        let mut reader = ogg::reading::PacketReader::new(Cursor::new(enc.finish().unwrap()));
        let _ = reader.read_packet().unwrap().unwrap();
        let _ = reader.read_packet().unwrap().unwrap();
        let setup = reader.read_packet().unwrap().unwrap();
        assert!(vorbis_lookup::VORBIS_LOOKUP.contains_key(&crc32fast::hash(&setup.data)));
        let mut audio = Vec::new();
        while let Some(packet) = reader.read_packet().unwrap() {
            audio.extend_from_slice(&(packet.data.len() as u16).to_le_bytes());
            audio.extend(packet.data);
        }
        audio.extend_from_slice(&0u16.to_le_bytes());
        audio.resize(audio.len().div_ceil(32)*32,0);
        let word = 1u64 | (8<<1) | (1<<5) | ((frames as u64)<<34);
        let mut headers = word.to_le_bytes().to_vec();
        let mut loop_data = 1000u32.to_le_bytes().to_vec(); loop_data.extend_from_slice(&40000u32.to_le_bytes());
        let mut codec = crc32fast::hash(&setup.data).to_le_bytes().to_vec();
        codec.extend_from_slice(&8u32.to_le_bytes()); codec.extend_from_slice(&[0;8]);
        let mut extras = vec![(3u8,loop_data),(11,codec)];
        if extra_unknown { extras.push((99,b"unrecognized encoded-offset metadata".to_vec())); }
        for (i,(kind,data)) in extras.iter().enumerate() {
            let h = (u32::from(*kind)<<25) | ((data.len() as u32)<<1) | u32::from(i+1<extras.len());
            headers.extend_from_slice(&h.to_le_bytes()); headers.extend(data);
        }
        let mut names = 4u32.to_le_bytes().to_vec(); names.extend_from_slice(b"test-audio\0");
        names.resize((60+headers.len()+names.len()).div_ceil(16)*16-60-headers.len(),0);
        let mut fsb = vec![0u8;60]; fsb[..4].copy_from_slice(b"FSB5");
        fsb[4..8].copy_from_slice(&1u32.to_le_bytes()); fsb[8..12].copy_from_slice(&1u32.to_le_bytes());
        fsb[12..16].copy_from_slice(&(headers.len() as u32).to_le_bytes());
        fsb[16..20].copy_from_slice(&(names.len() as u32).to_le_bytes());
        fsb[20..24].copy_from_slice(&(audio.len() as u32).to_le_bytes());
        fsb[24..28].copy_from_slice(&15u32.to_le_bytes());
        fsb[44..60].copy_from_slice(b"stable-sound-id!");
        fsb.extend(headers); fsb.extend(names); fsb.extend(audio); fsb
    }

    fn decode(sample: &Sample, data: &[u8]) -> Vec<f32> {
        let id = read_header_ident(&identification(sample.rate,sample.channels)).unwrap();
        let setup = read_header_setup(vorbis_lookup::VORBIS_LOOKUP.get(&sample.crc().unwrap()).unwrap(),sample.channels,(8,11)).unwrap();
        let mut window = PreviousWindowRight::new();
        let mut result = Vec::new(); let mut pos = 0;
        while pos+2 <= data.len() {
            let len = u16::from_le_bytes(data[pos..pos+2].try_into().unwrap()) as usize; pos += 2;
            if len == 0 || len == 65535 { break; }
            let pcm: Vec<Vec<f32>> = read_audio_packet_generic(&id,&setup,&data[pos..pos+len],&mut window).unwrap(); pos += len;
            result.extend_from_slice(&pcm[0]);
        }
        assert!(result.len() >= sample.frames); result.truncate(sample.frames); result
    }

    #[test]
    fn reencodes_fsb_and_preserves_names_loops_rate_frames_and_identity() {
        let original = fixture(false);
        let (output,changed,skipped) = rebuild_fsb(&original,0.0.into()).unwrap();
        assert_eq!((changed,skipped),(1,0)); assert!(output.len() < original.len());
        let old = parse(&original).unwrap(); let new = parse(&output).unwrap();
        assert_eq!(&old.header[44..60],&new.header[44..60]);
        assert_eq!(&old.names[..15],&new.names[..15]);
        assert_eq!(new.samples[0].frames,44100);
        assert_eq!(new.samples[0].rate,44100);
        assert_eq!(old.samples[0].extras[0],new.samples[0].extras[0]);
        let a = decode(&old.samples[0],old.audio); let b = decode(&new.samples[0],new.audio);
        let dot: f64 = a.iter().zip(&b).map(|(a,b)| f64::from(*a)*f64::from(*b)).sum();
        let power = |samples: &[f32]| samples.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
        assert!(dot/(power(&a)*power(&b)).sqrt() > 0.95,"audio timing/waveform correlation lost");
    }

    #[test]
    fn unknown_offset_metadata_is_not_rewritten() {
        let original = fixture(true);
        let (output,changed,skipped) = rebuild_fsb(&original,0.0.into()).unwrap();
        assert_eq!((changed,skipped),(0,1)); assert_eq!(output,original);
    }

    #[test]
    fn riff_event_metadata_is_unchanged() {
        let fsb = fixture(false);
        let mut riff = b"RIFF\0\0\0\0FEV LIST".to_vec();
        riff.extend_from_slice(&8u32.to_le_bytes()); riff.extend_from_slice(b"event-id");
        riff.extend_from_slice(b"SND "); riff.extend_from_slice(&(fsb.len() as u32+16).to_le_bytes());
        riff.extend_from_slice(&[0;16]); riff.extend(fsb);
        let size = riff.len() as u32-8; riff[4..8].copy_from_slice(&size.to_le_bytes());
        let (new,changed,_) = rebuild(&riff,0.0.into()).unwrap();
        assert_eq!(changed,1); assert_eq!(&new[8..32],&riff[8..32]);
        assert_eq!(u32_at(&new,4).unwrap() as usize+8,new.len());
        assert_eq!(u32_at(&new,32).unwrap() as usize+36,new.len());
    }

    #[test]
    fn malformed_fsb_sizes_offsets_and_riff_are_rejected() {
        let original = fixture(false);
        for at in [8,12,16,20] {
            let mut bad_input = original.clone(); bad_input[at..at+4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(parse(&bad_input).is_err());
        }
        assert!(rebuild(b"RIFF\xff\xff\xff\xffFEV ",0.0.into()).is_err());
        assert!(parse(b"FSB5").is_err());
    }

    #[test]
    fn export_never_overwrites_existing_files_or_source() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("bgc-fmod-export-{stamp}"));
        fs::create_dir(&root).unwrap();
        let original = fixture(false);
        let input = root.join("input.fsb"); let output = root.join("output.fsb");
        fs::write(&input,&original).unwrap(); fs::write(&output,b"keep this file").unwrap();
        assert!(run("0.0",&input,&output).is_err());
        assert_eq!(fs::read(&output).unwrap(),b"keep this file");
        assert!(run("0.0",&input,&input).is_err());
        assert_eq!(fs::read(&input).unwrap(),original);
        assert!(run("NaN",&input,&root.join("nan.fsb")).is_err());
        assert!(!root.join("nan.fsb").exists());
        audit(&input).unwrap();
        fs::write(root.join("malformed.bank"),b"RIFF\0\0\0\0FEV ").unwrap();
        assert!(audit(&root.join("malformed.bank")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quality_guard_rejects_damage_and_tracks_different_block_boundaries() {
        let pcm = vec![(0..1000).map(|i| (i as f32 * 0.03).sin()).collect::<Vec<_>>()];
        let mut check = QualityCheck::default();
        check.capture(&[pcm[0][..77].to_vec()],0,77);
        check.capture(&[pcm[0][77..].to_vec()],77,923);
        check.compare(&[pcm[0][..251].to_vec()],0,1000);
        check.compare(&[pcm[0][251..].to_vec()],251,1000);
        assert!(check.acceptable(20.0));
        assert_eq!(check.checked,32);
        let mut damaged = QualityCheck::default();
        damaged.capture(&pcm,0,1000);
        damaged.compare(&[vec![0.0;1000]],0,1000);
        assert!(!damaged.acceptable(15.0));
        assert!(!QualityCheck::default().acceptable(15.0));
    }

    #[test]
    fn conservative_profile_has_stronger_quality_and_gain_guards() {
        let conservative = policy("conservative").unwrap();
        let balanced = policy("balanced").unwrap();
        assert!(conservative.quality > balanced.quality);
        assert!(conservative.min_snr_db > balanced.min_snr_db);
        assert_eq!(conservative.min_gain_pct,5);
        for invalid in ["NaN","inf","-0.1","0.5","music"] {
            assert!(policy(invalid).is_err());
        }
        let original = fixture(false);
        let impossible = Policy { quality:0.0,min_snr_db:100.0,min_gain_pct:5 };
        let (new,changed,_) = rebuild_fsb(&original,impossible).unwrap();
        assert_eq!(changed,0);
        assert_eq!(new,original);
    }

    #[test]
    fn multi_sample_rebuild_relocates_retained_audio_without_changing_it() {
        let single = fixture(false);
        let one = parse(&single).unwrap();
        let hs = u32_at(&single,12).unwrap() as usize;
        let mut headers = single[60..60+hs].to_vec();
        let mut second = headers.clone();
        let word = one.samples[0].word | (((one.audio.len()/16) as u64)<<6);
        second[..8].copy_from_slice(&word.to_le_bytes());
        let mut pos = 8;
        while pos < second.len() {
            let extra = u32_at(&second,pos).unwrap(); pos += 4;
            let n = ((extra>>1)&0x00ff_ffff) as usize;
            if extra>>25 == 11 { second[pos..pos+4].copy_from_slice(&0xdeadbeefu32.to_le_bytes()); }
            pos += n;
        }
        headers.extend(second);
        let mut names = 8u32.to_le_bytes().to_vec();
        names.extend_from_slice(&14u32.to_le_bytes());
        names.extend_from_slice(b"first\0second\0");
        names.resize((60+headers.len()+names.len()).div_ceil(16)*16-60-headers.len(),0);
        let mut original = single[..60].to_vec();
        original[8..12].copy_from_slice(&2u32.to_le_bytes());
        original[12..16].copy_from_slice(&(headers.len() as u32).to_le_bytes());
        original[16..20].copy_from_slice(&(names.len() as u32).to_le_bytes());
        original[20..24].copy_from_slice(&(one.audio.len() as u32*2).to_le_bytes());
        original.extend(headers); original.extend(names);
        original.extend_from_slice(one.audio); original.extend_from_slice(one.audio);
        let (output,changed,skipped) = rebuild_fsb(&original,0.0.into()).unwrap();
        assert_eq!((changed,skipped),(1,1));
        let new = parse(&output).unwrap();
        assert_eq!(new.samples.len(),2);
        assert_eq!(new.samples[1].extras[0],one.samples[0].extras[0]);
        assert!(new.samples[1].offset < one.audio.len());
        assert_eq!(&new.audio[new.samples[1].offset..],one.audio);
        assert_eq!(&new.names[..21],&parse(&original).unwrap().names[..21]);
    }
}
