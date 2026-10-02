//! Bounded, read-only Godot PCK directory and Unreal Pak footer inspection.
//! Neither extension names nor footer codec names establish safe removability.
use std::{collections::{BTreeMap, BTreeSet}, fs::{self, File}, io::{self, Read, Write, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt}, path::Path};
use super::invalid;

fn u32le<R: Read>(r: &mut R) -> io::Result<u32> { let mut b = [0; 4]; r.read_exact(&mut b)?; Ok(u32::from_le_bytes(b)) }
fn u64le<R: Read>(r: &mut R) -> io::Result<u64> { let mut b = [0; 8]; r.read_exact(&mut b)?; Ok(u64::from_le_bytes(b)) }
fn godot_image_format(format: u32) -> &'static str {
    ["L8", "LA8", "R8", "RG8", "RGB8", "RGBA8", "RGBA4444", "RGB565",
     "RF", "RGF", "RGBF", "RGBAF", "RH", "RGH", "RGBH", "RGBAH", "RGBE9995",
     "DXT1_BC1", "DXT3_BC2", "DXT5_BC3", "RGTC_R_BC4", "RGTC_RG_BC5",
     "BPTC_RGBA_BC7", "BPTC_RGBF_BC6S", "BPTC_RGBFU_BC6U", "ETC", "ETC2_R11",
     "ETC2_R11S", "ETC2_RG11", "ETC2_RG11S", "ETC2_RGB8", "ETC2_RGBA8",
     "ETC2_RGB8A1", "ETC2_RA_AS_RG", "DXT5_RA_AS_RG", "ASTC_4x4", "ASTC_4x4_HDR",
     "ASTC_8x8", "ASTC_8x8_HDR"].get(format as usize).copied().unwrap_or("unknown")
}
fn rsrc_class<R: Read + Seek>(r: &mut R, offset: u64, size: u64) -> io::Result<Option<String>> {
    if size < 28 { return Ok(None); }
    r.seek(SeekFrom::Start(offset))?;
    let mut magic = [0; 4]; r.read_exact(&mut magic)?;
    if &magic != b"RSRC" { return Ok(None); }
    // Godot binary resource header: endian/real flags, engine version, format,
    // then a bounded length-prefixed resource class name.
    let endian = u32le(r)?;
    let _real64 = u32le(r)?;
    let major = u32le(r)?; let minor = u32le(r)?;
    let _format = u32le(r)?;
    let class_len = u32le(r)? as usize;
    if endian != 0 || major == 0 || major > 32 || minor > 100 || class_len == 0 || class_len > 256
        || (28u64 + class_len as u64) > size { return Ok(None); }
    let mut class = vec![0; class_len]; r.read_exact(&mut class)?;
    if class.last() == Some(&0) { class.pop(); }
    if class.is_empty() || !class.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_') { return Ok(None); }
    Ok(Some(String::from_utf8(class).map_err(|_| invalid("invalid Godot resource class"))?))
}
struct ResourceReader<'a, R> { file: &'a mut R, start: u64, size: u64, pos: u64 }
impl<R: Read + Seek> ResourceReader<'_, R> {
    fn seek(&mut self, pos: u64) -> io::Result<()> {
        if pos > self.size { return Err(invalid("Godot resource offset outside entry")); }
        self.file.seek(SeekFrom::Start(self.start.checked_add(pos).ok_or_else(|| invalid("Godot resource offset overflow"))?))?;
        self.pos = pos; Ok(())
    }
    fn read(&mut self, buf: &mut [u8]) -> io::Result<()> {
        let end = self.pos.checked_add(buf.len() as u64).ok_or_else(|| invalid("Godot resource read overflow"))?;
        if end > self.size { return Err(invalid("truncated Godot resource")); }
        self.file.read_exact(buf)?; self.pos = end; Ok(())
    }
    fn skip(&mut self, len: u64) -> io::Result<()> {
        let end = self.pos.checked_add(len).ok_or_else(|| invalid("Godot resource skip overflow"))?;
        self.seek(end)
    }
    fn u32(&mut self) -> io::Result<u32> { let mut b = [0; 4]; self.read(&mut b)?; Ok(u32::from_le_bytes(b)) }
    fn u64(&mut self) -> io::Result<u64> { let mut b = [0; 8]; self.read(&mut b)?; Ok(u64::from_le_bytes(b)) }
    fn string(&mut self) -> io::Result<String> {
        let len = self.u32()? as usize;
        if len == 0 || len > 4096 || len as u64 > self.size.saturating_sub(self.pos) { return Err(invalid("invalid Godot resource string length")); }
        let mut b = vec![0; len]; self.read(&mut b)?;
        if b.last() == Some(&0) { b.pop(); }
        String::from_utf8(b).map_err(|_| invalid("invalid UTF-8 in Godot resource"))
    }
}
#[derive(Default)]
struct WavInfo { format: u32, stereo: bool, rate: u32, loop_mode: u32, loop_begin: u32, loop_end: u32, data_bytes: u64 }
fn wav_resource<R: Read + Seek>(file: &mut R, offset: u64, size: u64) -> io::Result<Option<WavInfo>> {
    if size < 32 || size > 64 * 1024 * 1024 { return Ok(None); }
    let mut r = ResourceReader { file, start: offset, size, pos: 0 };
    r.seek(0)?;
    let mut magic = [0; 4]; r.read(&mut magic)?;
    if &magic != b"RSRC" { return Ok(None); }
    let endian = r.u32()?; let _real64 = r.u32()?;
    let major = r.u32()?; let minor = r.u32()?; let format = r.u32()?;
    if endian != 0 || major != 4 || minor > 99 || format > 6 { return Ok(None); }
    let class = r.string()?;
    if class != "AudioStreamWAV" { return Ok(None); }
    let _metadata_offset = r.u64()?;
    let flags = r.u32()?; let _uid = r.u64()?;
    if flags & !15 != 0 { return Ok(None); }
    if flags & 8 != 0 { let _script_class = r.string()?; }
    for _ in 0..11 { let _reserved = r.u32()?; }
    let string_count = r.u32()? as usize;
    if string_count > 8192 { return Ok(None); }
    let mut strings = Vec::with_capacity(string_count);
    for _ in 0..string_count { strings.push(r.string()?); }
    let external_count = r.u32()? as usize;
    if external_count > 8192 { return Ok(None); }
    for _ in 0..external_count {
        let _kind = r.string()?; let _path = r.string()?;
        if flags & 2 != 0 { let _uid = r.u64()?; }
    }
    let internal_count = r.u32()? as usize;
    if internal_count == 0 || internal_count > 8192 { return Ok(None); }
    let mut main_offset = None;
    for _ in 0..internal_count {
        let _path = r.string()?; let resource_offset = r.u64()?;
        if resource_offset >= size { return Ok(None); }
        main_offset = Some(resource_offset);
    }
    r.seek(main_offset.unwrap())?;
    if r.string()? != "AudioStreamWAV" { return Ok(None); }
    let property_count = r.u32()? as usize;
    if property_count > 256 { return Ok(None); }
    let mut info = WavInfo { rate: 44100, ..WavInfo::default() };
    let mut saw_data = false;
    for _ in 0..property_count {
        let key_index = r.u32()? as usize;
        let key = match strings.get(key_index) { Some(v) => v.as_str(), None => return Ok(None) };
        let tag = r.u32()?;
        match tag {
            1 => {}, // Variant::NIL
            2 => { let value = r.u32()?; if key == "stereo" { info.stereo = value != 0; } },
            3 => {
                let value = r.u32()?;
                match key {
                    "format" => info.format = value,
                    "loop_mode" => info.loop_mode = value,
                    "loop_begin" => info.loop_begin = value,
                    "loop_end" => info.loop_end = value,
                    "mix_rate" => info.rate = value,
                    _ => {},
                }
            }
            4 => { let _value = r.u32()?; }
            5 => { let _value = r.string()?; }
            31 if key == "data" => {
                let len = r.u32()? as u64; info.data_bytes = len; saw_data = true;
                let pad = (4 - len % 4) % 4;
                r.skip(len.checked_add(pad).ok_or_else(|| invalid("Godot audio data length overflow"))?)?;
            }
            _ => return Ok(None),
        }
    }
    if !saw_data || info.format > 3 || info.loop_mode > 3 || info.rate == 0 || info.rate > 384_000 { return Ok(None); }
    Ok(Some(info))
}
struct Entry { name: String, name_raw: Vec<u8>, offset: u64, size: u64, flags: u32, hash: [u8; 16], offset_field: u64 }
struct Pack { version: u32, engine: [u32; 3], entries: Vec<Entry>, payload: u64 }

fn pck<R: Read + Seek>(f: &mut R, length: u64) -> io::Result<Pack> {
    if u32le(f)? != 0x43504447 { return Err(invalid("not a standalone Godot PCK")); }
    let version = u32le(f)?;
    if !(1..=4).contains(&version) { return Err(invalid("unsupported Godot PCK version")); }
    let engine = [u32le(f)?, u32le(f)?, u32le(f)?];
    let (base, directory) = if version == 1 { (0, 84) } else {
        let flags = u32le(f)?;
        if flags & !2 != 0 { return Err(invalid("encrypted/sparse/unknown Godot PCK layout unsupported")); }
        let base = u64le(f)?;
        (base, if version >= 3 { u64le(f)? } else { 96 })
    };
    if base > length || directory.checked_add(4).filter(|v| *v <= length).is_none() {
        return Err(invalid("Godot PCK header offsets outside file"));
    }
    f.seek(SeekFrom::Start(directory))?;
    let count = u32le(f)? as usize;
    if count > 200_000 { return Err(invalid("Godot PCK file-count limit exceeded")); }
    let mut entries = Vec::with_capacity(count);
    let mut payload = 0u64;
    for _ in 0..count {
        super::cancelled()?;
        if f.stream_position()?.saturating_sub(directory) > 64 * 1024 * 1024 { return Err(invalid("Godot PCK directory budget exceeded")); }
        let n = u32le(f)? as usize;
        if n == 0 || n > 4096 { return Err(invalid("invalid Godot PCK path length")); }
        let mut name_raw = vec![0; n]; f.read_exact(&mut name_raw)?;
        let mut trimmed = name_raw.clone();
        while trimmed.last() == Some(&0) { trimmed.pop(); }
        let name = String::from_utf8(trimmed).map_err(|_| invalid("invalid UTF-8 Godot resource path"))?;
        if name.is_empty() || name.chars().any(|c| c.is_control() || c == '|') { return Err(invalid("unsafe Godot report path")); }
        let offset_field = f.stream_position()?;
        let relative = u64le(f)?;
        let size = u64le(f)?;
        let mut hash = [0; 16]; f.read_exact(&mut hash)?; // No checksum claim: payloads are not fully read.
        let flags = if version >= 2 { u32le(f)? } else { 0 };
        if flags & !3 != 0 { return Err(invalid("unknown Godot PCK entry flags")); }
        let offset = base.checked_add(relative).ok_or_else(|| invalid("Godot PCK offset overflow"))?;
        if flags & 2 == 0 {
            if offset.checked_add(size).filter(|v| *v <= length).is_none() { return Err(invalid("Godot PCK entry extent outside file")); }
            payload = payload.checked_add(size).ok_or_else(|| invalid("Godot PCK payload sum overflow"))?;
        }
        entries.push(Entry { name, name_raw, offset, size, flags, hash, offset_field });
    }
    let directory_end = f.stream_position()?;
    if directory_end > length { return Err(invalid("Godot directory exceeds file")); }
    for e in &entries {
        if e.flags & 2 == 0 && e.size > 0 && e.offset < directory_end && e.offset + e.size > directory {
            return Err(invalid("Godot resource overlaps directory"));
        }
    }
    Ok(Pack { version, engine, entries, payload })
}

fn open(path: &Path) -> io::Result<(fs::File, fs::Metadata)> {
    let file = fs::OpenOptions::new().read(true).custom_flags(0x20000 | 0x800).open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() { return Err(invalid("container audit expects a regular file")); }
    Ok((file, meta))
}
fn unchanged(before: &fs::Metadata, after: &fs::Metadata) -> io::Result<()> {
    if before.len() != after.len() || before.mtime() != after.mtime() || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime() || before.ctime_nsec() != after.ctime_nsec() {
        return Err(invalid("container changed during audit"));
    }
    Ok(())
}

pub fn audit(path: &Path) -> io::Result<()> {
    let (mut file, _) = open(path)?;
    let mut prefix = [0; 20];
    let n = file.read(&mut prefix)?;
    if n < 4 { return Err(invalid("truncated container signature")); }
    match &prefix[..4] {
        b"GDPC" => godot_audit(path),
        b"Unit" => super::unityfs::run(path, None, 5.0),
        _ if super::unity_serialized::plausible(&prefix[..n]) => super::unity_serialized::audit(path),
        _ => unreal_audit(path),
    }
}

pub fn godot_audit(path: &Path) -> io::Result<()> {
    let (mut file, meta) = open(path)?;
    let pack = pck(&mut file, meta.len())?;
    let mut types: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut resources: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    let mut audio: BTreeMap<(u32, bool, u32, u32), (u64, u64, u64, u64)> = BTreeMap::new();
    let mut audio_unparsed = 0u64;
    // Encoding and GPU Image::Format are separate: "raw-image" may contain BCn/ETC data.
    let mut textures: BTreeMap<(u32, u32), (u64, u64, u32, u32, u64)> = BTreeMap::new();
    for e in &pack.entries {
        super::cancelled()?;
        if e.flags & 2 != 0 { continue; }
        let ext = e.name.rsplit_once('.').map(|(_, v)| v.to_ascii_lowercase()).unwrap_or_else(|| "none".into());
        let slot = types.entry(ext.clone()).or_default(); slot.0 += 1; slot.1 += e.size;
        if ext == "sample" && e.flags & 1 == 0 {
            file.seek(SeekFrom::Start(e.offset))?;
            if let Some(class) = rsrc_class(&mut file, e.offset, e.size)? {
                let slot = resources.entry((ext.clone(), class)).or_default();
                slot.0 += 1; slot.1 += e.size;
            }
            match if e.size <= 64 * 1024 * 1024 { wav_resource(&mut file, e.offset, e.size) } else { Ok(None) } {
                Ok(Some(info)) => {
                    let slot = audio.entry((info.format, info.stereo, info.rate, info.loop_mode)).or_default();
                    slot.0 += 1; slot.1 += e.size; slot.2 += info.data_bytes;
                    if info.loop_mode != 0 { slot.3 += 1; }
                }
                Ok(None) => audio_unparsed += 1,
                Err(err) if err.kind() == io::ErrorKind::InvalidData => audio_unparsed += 1,
                Err(err) => return Err(err),
            }
        }
        if ext == "ctex" && e.flags & 1 == 0 && e.size >= 52 {
            file.seek(SeekFrom::Start(e.offset))?;
            let mut b = [0; 52]; file.read_exact(&mut b)?;
            let word = |p| u32::from_le_bytes(b[p..p + 4].try_into().unwrap());
            if &b[..4] == b"GST2" && word(4) <= 1 {
                let encoding = word(36); let format = word(48);
                let w = u16::from_le_bytes(b[40..42].try_into().unwrap()) as u32;
                let h = u16::from_le_bytes(b[42..44].try_into().unwrap()) as u32;
                let mipmaps = word(44);
                if encoding <= 3 && w > 0 && h > 0 && mipmaps <= 16 {
                    let slot = textures.entry((encoding, format)).or_default();
                    slot.0 += 1; slot.1 += e.size; slot.2 = slot.2.max(w); slot.3 = slot.3.max(h);
                    if mipmaps > 0 { slot.4 += 1; }
                }
            }
        }
    }
    unchanged(&meta, &file.metadata()?)?;
    println!("GODOT_PCK|{}|{}.{}.{}|{}|{}|{}|{}", pack.version, pack.engine[0], pack.engine[1], pack.engine[2],
        meta.len(), pack.entries.len(), pack.payload, pack.entries.iter().filter(|e| e.flags & 1 != 0).count());
    for (ext, (count, bytes)) in types { println!("PCK_TYPE|{ext}|{count}|{bytes}"); }
    for ((ext, class), (count, bytes)) in resources { println!("PCK_RESOURCE|{ext}|{class}|{count}|{bytes}"); }
    for ((format, stereo, rate, loop_mode), (count, bytes, data, loops)) in audio {
        let format_name = ["8-bit", "16-bit", "IMA-ADPCM", "QOA"][format as usize];
        println!("PCK_AUDIO|{format_name}|{}|{rate}|{loop_mode}|{count}|{bytes}|{data}|{loops}", if stereo { "stereo" } else { "mono" });
    }
    println!("PCK_AUDIO_UNPARSED|{audio_unparsed}");
    let mut named_formats = BTreeMap::new();
    for ((encoding, format), (count, bytes, w, h, mips)) in textures {
        let encoding = ["raw-image", "png", "webp", "basis"][encoding as usize];
        println!("PCK_TEXTURE|{encoding}|{format}|{count}|{bytes}|{w}|{h}|{mips}");
        if named_formats.insert(format, ()).is_none() { println!("PCK_IMAGE_FORMAT|{format}|{}", godot_image_format(format)); }
    }
    eprintln!("Read-only Godot inventory: bounded resource/audio metadata only; payload bytes are skipped, not verified or rewritten. No pruning decisions or savings are implied, and PCK entries cannot be arbitrarily switched to Zstd.");
    Ok(())
}

struct Footer { version: u32, offset: u64, size: u64, encrypted: bool, frozen: bool, hash: [u8; 20], codecs: Vec<String> }
fn footer(b: &[u8], file_length: u64) -> io::Result<Footer> {
    let magic = 0x5a6f12e1u32.to_le_bytes();
    let mut matches = Vec::new();
    for pos in 0..b.len().saturating_sub(43) {
        if b[pos..pos + 4] != magic { continue; }
        let mut c = io::Cursor::new(&b[pos + 4..]);
        let version = u32le(&mut c)?;
        if !(1..=11).contains(&version) { continue; }
        let prefix = if version >= 7 { 17 } else if version >= 4 { 1 } else { 0 };
        if pos < prefix { continue; }
        let method_counts: &[usize] = if version == 8 { &[4, 5] } else if version >= 9 { &[5] } else { &[0] };
        for count in method_counts {
            let frozen = usize::from(version == 9);
            if pos + 44 + frozen + count * 32 != b.len() { continue; }
            let offset = u64::from_le_bytes(b[pos + 8..pos + 16].try_into().unwrap());
            let size = u64::from_le_bytes(b[pos + 16..pos + 24].try_into().unwrap());
            let footer_start = file_length - b.len() as u64 + (pos - prefix) as u64;
            if offset.checked_add(size).filter(|v| *v <= footer_start).is_none() { continue; }
            let encrypted = if version >= 4 {
                if b[pos - 1] > 1 { continue; } b[pos - 1] == 1
            } else { false };
            if frozen != 0 && b[pos + 44] > 1 { continue; }
            let mut codecs = Vec::new(); let mut valid = true;
            for slot in b[pos + 44 + frozen..].chunks_exact(32) {
                let n = slot.iter().position(|v| *v == 0).unwrap_or(32);
                if slot[n..].iter().any(|v| *v != 0) || !slot[..n].iter().all(|v| v.is_ascii_alphanumeric() || *v == b'_') {
                    valid = false; break;
                }
                if n > 0 { codecs.push(String::from_utf8(slot[..n].to_vec()).unwrap()); }
            }
            if valid { matches.push(Footer { version, offset, size, encrypted,
                frozen: frozen != 0 && b[pos + 44] != 0,
                hash: b[pos + 24..pos + 44].try_into().unwrap(), codecs }); }
        }
    }
    if matches.len() != 1 { return Err(invalid("unsupported/ambiguous Unreal Pak footer (not a generic .pak parser)")); }
    Ok(matches.remove(0))
}

pub fn unreal_audit(path: &Path) -> io::Result<()> {
    let (mut file, meta) = open(path)?;
    let n = meta.len().min(1024) as usize;
    file.seek(SeekFrom::End(-(n as i64)))?;
    let mut b = vec![0; n]; file.read_exact(&mut b)?;
    let f = footer(&b, meta.len())?;
    let index_status = verify_unreal_index(&mut file, &f)?;
    let entries = if index_status == "VERIFIED_PRIMARY_SHA1" && f.version <= 7 && f.size <= 32 * 1024 * 1024 {
        Some(super::unreal_legacy::inspect(&mut file, f.version, f.offset, f.size)?)
    } else { None };
    // A companion signature's presence is a blocker for future writers, not
    // proof of authenticity. Do not read/execute it or follow companion links.
    let signature = path.with_extension("sig");
    let signed = match fs::symlink_metadata(signature) {
        Ok(_) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e),
    };
    unchanged(&meta, &file.metadata()?)?;
    let codecs = if f.version < 8 { "legacy-method-IDs".into() } else { f.codecs.join(",") };
    println!("UNREAL_PAK|{}|{}|{}|{}|{}|{}", f.version, meta.len(), f.offset, f.size, u8::from(f.encrypted), codecs);
    println!("UNREAL_INDEX|{index_status}|{}", f.size);
    println!("UNREAL_SECURITY|{}|{}|{}", u8::from(f.encrypted), u8::from(signed), u8::from(f.frozen));
    if let Some(r) = entries {
        println!("UNREAL_LEGACY_ENTRIES|{}|{}|{}|{}|{}|{}|{}", r.entries, r.stored, r.encrypted,
            r.deleted, r.header_checked, r.payload_checked, r.payload_skipped);
        for (id, count) in r.methods { println!("UNREAL_METHOD_ID|{id}|{count}"); }
        for (kind, count) in r.kinds { println!("UNREAL_ENTRY_KIND|{kind}|{count}"); }
    } else {
        println!("UNREAL_LEGACY_ENTRIES|SKIPPED_VERSION_ENCRYPTION_OR_BUDGET");
    }
    eprintln!("Read-only Unreal Pak: primary SHA1 is not signature verification. Bounded legacy v1–7 audits inspect indexed methods, data headers and eligible stored payload hashes; compressed/encrypted payloads and modern/secondary indexes remain unverified. No texture/audio writer or Pak/IoStore repacker is implemented; RE Engine archives are not Unreal Paks.");
    Ok(())
}

fn verify_unreal_index<R: Read + Seek>(file: &mut R, footer: &Footer) -> io::Result<&'static str> {
    use sha1::{Digest, Sha1};
    if footer.encrypted { return Ok("SKIPPED_ENCRYPTED"); }
    if footer.size > 256 * 1024 * 1024 { return Ok("SKIPPED_BUDGET"); }
    let mut hash = Sha1::new();
    file.seek(SeekFrom::Start(footer.offset))?;
    let mut remaining = footer.size; let mut b = [0u8; 65536];
    while remaining > 0 {
        super::cancelled()?;
        let n = remaining.min(b.len() as u64) as usize;
        file.read_exact(&mut b[..n])?; hash.update(&b[..n]); remaining -= n as u64;
    }
    let digest: [u8; 20] = hash.finalize().into();
    if digest != footer.hash { return Err(invalid("Unreal Pak primary index SHA1 mismatch")); }
    Ok("VERIFIED_PRIMARY_SHA1")
}

// A stored MD5 is only a grouping hint. Every shared payload is compared byte
// for byte; a collision, corrupt hash, or different path never justifies removal.
fn equal_ranges<R: Read + Seek>(file: &mut R, a: u64, b: u64, size: u64, budget: &mut u64) -> io::Result<bool> {
    let mut left = [0u8; 65536]; let mut right = [0u8; 65536]; let mut done = 0;
    while done < size {
        super::cancelled()?;
        let n = (size - done).min(left.len() as u64) as usize;
        *budget = budget.checked_sub(n as u64 * 2).ok_or_else(|| invalid("Godot duplicate comparison budget exceeded"))?;
        file.seek(SeekFrom::Start(a + done))?; file.read_exact(&mut left[..n])?;
        file.seek(SeekFrom::Start(b + done))?; file.read_exact(&mut right[..n])?;
        if left[..n] != right[..n] { return Ok(false); }
        done += n as u64;
    }
    Ok(true)
}

struct Dedup { removed: Vec<(u64, u64)>, prefix: Vec<u64>, aliases: BTreeMap<u64, u64>, saved: u64 }
fn dedup_plan<R: Read + Seek>(file: &mut R, pack: &Pack) -> io::Result<Dedup> {
    let mut extents: BTreeMap<(u64, u64), &Entry> = BTreeMap::new();
    for e in &pack.entries { if e.size > 0 && e.flags & 2 == 0 { extents.entry((e.offset, e.size)).or_insert(e); } }
    let mut end = 0;
    for (&(off, size), _) in &extents {
        if off < end { return Err(invalid("partially overlapping PCK entries; cannot safely compact")); }
        end = off + size;
    }
    let mut groups: BTreeMap<([u8; 16], u64), Vec<u64>> = BTreeMap::new();
    let mut removed = Vec::new(); let mut aliases = BTreeMap::new(); let mut saved = 0;
    let mut budget = 32u64 * 1024 * 1024 * 1024;
    for ((offset, size), e) in extents {
        if e.flags != 0 || e.hash == [0; 16] { continue; }
        let group = groups.entry((e.hash, size)).or_default();
        let mut duplicate = None;
        // Refuse pathological hash buckets instead of quadratic disk reads.
        if group.len() >= 64 { return Err(invalid("Godot duplicate hash bucket exceeds work limit")); }
        for &previous in group.iter() {
            if equal_ranges(file, previous, offset, size, &mut budget)? { duplicate = Some(previous); break; }
        }
        if let Some(previous) = duplicate {
            removed.push((offset, size)); aliases.insert(offset, previous); saved += size;
        } else { group.push(offset); }
    }
    let mut sum = 0;
    let prefix = removed.iter().map(|&(_, size)| { sum += size; sum }).collect();
    Ok(Dedup { removed, prefix, aliases, saved })
}
fn shifted(offset: u64, plan: &Dedup) -> io::Result<u64> {
    let n = plan.removed.partition_point(|&(start, size)| start + size <= offset);
    if let Some(&(start, _)) = plan.removed.get(n) {
        if offset >= start { return Err(invalid("PCK metadata points into removed extent")); }
    }
    let shrink = if n == 0 { 0 } else { plan.prefix[n - 1] };
    Ok(offset - shrink)
}
fn copy_range(file: &mut fs::File, target: &mut fs::File, start: u64, size: u64) -> io::Result<()> {
    file.seek(SeekFrom::Start(start))?; let mut b = [0; 65536]; let mut remaining = size;
    while remaining > 0 {
        super::cancelled()?; let n = remaining.min(b.len() as u64) as usize;
        file.read_exact(&mut b[..n])?; target.write_all(&b[..n])?; remaining -= n as u64;
    }
    Ok(())
}

/// Streaming, export-only PCK deduplication: same paths, flags, lengths, hashes
/// and bytes for every resource; duplicate payloads share an offset. All gaps,
/// opaque header bytes, directory placement and trailers are otherwise kept.
pub fn godot_dedup(input: &Path, output: Option<&Path>, min_efficiency: f64) -> io::Result<()> {
    if !min_efficiency.is_finite() || !(0.0..=100.0).contains(&min_efficiency) {
        return Err(invalid("Godot write-efficiency percentage must be between 0 and 100"));
    }
    if let Some(path) = output {
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "PCK destination already exists")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (), Err(e) => return Err(e),
        }
    }
    let (mut file, meta) = open(input)?;
    let pack = pck(&mut file, meta.len())?;
    // Do not reinterpret encrypted resource envelopes or removal markers.
    if pack.entries.iter().any(|e| e.flags != 0) { return Err(invalid("PCK dedup requires plain entries without removal flags")); }
    let plan = dedup_plan(&mut file, &pack)?;
    unchanged(&meta, &file.metadata()?)?;
    let after = meta.len() - plan.saved;
    let efficiency = plan.saved as f64 * 100.0 / after.max(1) as f64;
    let status = if plan.saved == 0 { "NO_GAIN" } else if efficiency < min_efficiency { "LOW_EFFICIENCY" } else if let Some(path) = output {
        let base = if pack.version >= 2 { file.seek(SeekFrom::Start(24))?; u64le(&mut file)? } else { 0 };
        // Header/file-base must stay in the preserved prefix.
        if plan.removed.iter().any(|&(start, _)| start < base.max(100)) { return Err(invalid("unsupported PCK payload/header placement")); }
        let directory = if pack.version >= 3 { file.seek(SeekFrom::Start(32))?; Some(u64le(&mut file)?) } else { None };
        let new_base = shifted(base, &plan)?;
        let mut target = fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).custom_flags(0x20000).open(path)?;
        let write = (|| {
            let mut pos = 0;
            for &(start, size) in &plan.removed { copy_range(&mut file, &mut target, pos, start - pos)?; pos = start + size; }
            copy_range(&mut file, &mut target, pos, meta.len() - pos)?;
            for e in &pack.entries {
                let canonical = plan.aliases.get(&e.offset).copied().unwrap_or(e.offset);
                let offset = shifted(canonical, &plan)?.checked_sub(new_base).ok_or_else(|| invalid("invalid compacted PCK base"))?;
                target.seek(SeekFrom::Start(shifted(e.offset_field, &plan)?))?; target.write_all(&offset.to_le_bytes())?;
            }
            if let Some(directory) = directory { target.seek(SeekFrom::Start(32))?; target.write_all(&shifted(directory, &plan)?.to_le_bytes())?; }
            target.seek(SeekFrom::Start(0))?;
            let check = pck(&mut target, after)?;
            if check.version != pack.version || check.engine != pack.engine || check.entries.len() != pack.entries.len() {
                return Err(invalid("exported PCK header verification failed"));
            }
            let mut verify_budget = 128u64 * 1024 * 1024 * 1024;
            // Verify every entry in the final exported file against the source,
            // even entries not changed by deduplication, with bounded buffers.
            let mut a = [0u8; 65536]; let mut b = [0u8; 65536];
            for (old, new) in pack.entries.iter().zip(&check.entries) {
                if old.name != new.name || old.size != new.size || old.hash != new.hash || old.flags != new.flags {
                    return Err(invalid("exported PCK resource directory changed"));
                }
                let mut done = 0;
                while done < old.size {
                    super::cancelled()?; let n = (old.size - done).min(a.len() as u64) as usize;
                    verify_budget = verify_budget.checked_sub(n as u64 * 2).ok_or_else(|| invalid("PCK verification budget exceeded"))?;
                    file.seek(SeekFrom::Start(old.offset + done))?; file.read_exact(&mut a[..n])?;
                    target.seek(SeekFrom::Start(new.offset + done))?; target.read_exact(&mut b[..n])?;
                    if a[..n] != b[..n] { return Err(invalid("exported PCK resource bytes differ")); } done += n as u64;
                }
            }
            unchanged(&meta, &file.metadata()?)?; super::cancelled()?; target.sync_all()
        })();
        if let Err(e) = write { let _ = fs::remove_file(path); return Err(e); }
        "EXPORTED"
    } else { "CANDIDATE" };
    println!("GODOT_DEDUP|{}|{}|{}|{}|{:.4}|{}", meta.len(), after, pack.entries.len(), plan.removed.len(), efficiency, status);
    eprintln!("Source unchanged. Equal resource bytes verified; logical savings only. Exported PCK still needs physical measurement and game loading validation.");
    Ok(())
}

fn godot3_vorbis_quality(profile: &str) -> io::Result<Option<f32>> {
    Ok(crate::audio_policy::target_for(profile)?.map(|target| target.vorbis_quality))
}

/// Export-only Godot 3 (standalone PCK v1) audio optimization. `AudioStreamMP3`
/// resources are decoded and re-encoded as Ogg Vorbis under the same entry name,
/// and the matching `.import` stub is retyped so the engine still resolves the
/// resource. PCM samples are reduced in rate/bit depth according to the profile,
/// preserving their resource class. Every other entry is copied byte for byte; the finished export is
/// re-parsed and untouched entries are byte-compared against the source.
pub fn godot3_optimize(input: &Path, output: Option<&Path>, min_efficiency: f64, profile: &str) -> io::Result<()> {
    godot3_transform(input, output, min_efficiency, profile, false)
}

/// Extract literal Godot resource paths from plain text/binary metadata. This
/// does not attempt to decode compressed resources or infer atlas geometry.
fn resource_paths(bytes: &[u8]) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for (i, window) in bytes.windows(6).enumerate() {
        if window != b"res://" { continue; }
        let tail = &bytes[i + 6..];
        let end = tail.iter().position(|b| *b < 32 || matches!(*b, b'"' | b'\''))
            .unwrap_or(tail.len());
        if end == 0 || end > 4096 { continue; }
        if let Ok(path) = std::str::from_utf8(&tail[..end]) { paths.insert(path.to_owned()); }
    }
    paths
}

/// Protect all aliases of named atlases and of textures referenced by plain
/// AtlasTexture resources. Resolve source texture paths through .import stubs
/// to their encoded .ctex/.stex payloads. Unrecognized atlases remain a caveat.
fn protected_texture_offsets(file: &mut File, pack: &Pack) -> io::Result<BTreeSet<u64>> {
    let mut paths = BTreeSet::new();
    let mut budget = 64u64 * 1024 * 1024;
    for e in &pack.entries {
        if crate::texture_policy::atlas_hint(Path::new(&e.name)) {
            paths.insert(e.name.trim_start_matches("res://").to_owned());
        }
        let ext = Path::new(&e.name).extension().and_then(|s| s.to_str()).unwrap_or("");
        if !matches!(ext, "tres" | "res" | "tscn" | "scn") { continue; }
        // Fail the packed transform closed rather than silently omitting a
        // potentially important atlas declaration when metadata exceeds bounds.
        if e.size > 4 * 1024 * 1024 { return Err(invalid("atlas metadata exceeds inspection limit")); }
        budget = budget.checked_sub(e.size).ok_or_else(|| invalid("atlas metadata inspection budget exceeded"))?;
        let mut bytes = vec![0; e.size as usize];
        file.seek(SeekFrom::Start(e.offset))?; file.read_exact(&mut bytes)?;
        if bytes.windows(12).any(|w| w == b"AtlasTexture") {
            paths.extend(resource_paths(&bytes));
        }
        super::cancelled()?;
    }
    // Use a snapshot: one import indirection resolves source images to their
    // runtime payload; no recursive graph traversal or path guessing is needed.
    let sources = paths.clone();
    for e in &pack.entries {
        let name = e.name.trim_start_matches("res://");
        if !name.strip_suffix(".import").is_some_and(|source| sources.contains(source)) { continue; }
        if e.size > 1024 * 1024 { return Err(invalid("atlas import metadata exceeds inspection limit")); }
        budget = budget.checked_sub(e.size).ok_or_else(|| invalid("atlas metadata inspection budget exceeded"))?;
        let mut bytes = vec![0; e.size as usize];
        file.seek(SeekFrom::Start(e.offset))?; file.read_exact(&mut bytes)?;
        paths.extend(resource_paths(&bytes));
        super::cancelled()?;
    }
    Ok(pack.entries.iter().filter(|e| paths.contains(e.name.trim_start_matches("res://")))
        .map(|e| e.offset).collect())
}

/// In-place Godot 3 PCK audio/texture rewrite used by the main asset pipeline.
///
/// Chains the two export transforms — audio (MP3→Vorbis, PCM resample, `.import`
/// retyping) and GDST `.stex` texture downscale — into sibling temporary files,
/// then recompresses and atomically renames the final result over the pack. The
/// texture pass reads the audio result when there is one, so both shrinks
/// compose in a single pass. No per-file backup is kept, matching the Steam
/// "Verify integrity of game files" recovery path. Returns `(textures, audio)`,
/// the number of categories actually rewritten (0 or 1 each).
pub fn godot3_apply(pack: &Path, min_efficiency: f64, profile: &str, level: u8) -> io::Result<(u64, u64)> {
    let audio_tmp = sibling_temp(pack, "audio");
    let texture_tmp = sibling_temp(pack, "tex");
    let _ = fs::remove_file(&audio_tmp);
    let _ = fs::remove_file(&texture_tmp);
    let mut audio_written = false;
    let mut texture_written = false;
    let write = (|| -> io::Result<()> {
        godot3_optimize(pack, Some(&audio_tmp), min_efficiency, profile)?;
        audio_written = fs::symlink_metadata(&audio_tmp).is_ok();
        // The transform writes nothing when its savings gate rejects the pack.
        let tex_input = if audio_written { audio_tmp.as_path() } else { pack };
        godot3_transform(tex_input, Some(&texture_tmp), min_efficiency, profile, true)?;
        texture_written = fs::symlink_metadata(&texture_tmp).is_ok();
        let winner = if texture_written {
            &texture_tmp
        } else if audio_written {
            &audio_tmp
        } else {
            return Ok(());
        };
        let m = fs::metadata(pack)?;
        fs::set_permissions(winner, m.permissions())?;
        let f = fs::OpenOptions::new().read(true).open(winner)?;
        super::compress_file_best_effort(&f, pack, level)?;
        drop(f);
        File::open(winner.parent().ok_or_else(|| invalid("invalid PCK path"))?)?.sync_all()?;
        fs::rename(winner, pack)?;
        File::open(pack.parent().ok_or_else(|| invalid("invalid PCK path"))?)?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(&audio_tmp);
    let _ = fs::remove_file(&texture_tmp);
    write?;
    Ok((texture_written as u64, audio_written as u64))
}

fn godot3_transform(input: &Path, output: Option<&Path>, min_efficiency: f64, profile: &str, textures_only: bool) -> io::Result<()> {
    if !min_efficiency.is_finite() || !(0.0..=100.0).contains(&min_efficiency) {
        return Err(invalid("Godot write-efficiency percentage must be between 0 and 100"));
    }
    let quality = match godot3_vorbis_quality(profile)? {
        Some(q) => q,
        None => {
            println!("GODOT3|native|0|0|0|0|0|0|0|0.0000|NO_PROFILE");
            eprintln!("Native/Lossless never rewrites Godot 3 audio; no output was written.");
            return Ok(());
        }
    };
    if let Some(path) = output {
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "PCK destination already exists")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (), Err(e) => return Err(e),
        }
    }
    let (mut file, meta) = open(input)?;
    file.seek(SeekFrom::Start(0))?;
    let magic = u32le(&mut file)?;
    let version = u32le(&mut file)?;
    if magic != 0x43504447 || version != 1 {
        return Err(invalid("Godot 3 audio rewriting requires a standalone PCK v1 file"));
    }
    file.seek(SeekFrom::Start(0))?;
    let pack = pck(&mut file, meta.len())?;
    if pack.entries.iter().any(|e| e.flags != 0) {
        return Err(invalid("Godot 3 rewriting requires plain entries"));
    }
    let protected = if textures_only { protected_texture_offsets(&mut file, &pack)? } else { BTreeSet::new() };
    // v1 keeps the directory immediately after its 84-byte header and stores
    // absolute payload offsets, so the structural prefix is fixed length.
    let directory = 84u64;
    let mut dir_end = directory + 4;
    for e in &pack.entries {
        dir_end = dir_end
            .checked_add(4 + e.name_raw.len() as u64 + 8 + 8 + 16)
            .ok_or_else(|| invalid("PCK directory overflow"))?;
    }
    if dir_end >= meta.len() { return Err(invalid("unsupported Godot 3 PCK layout")); }
    let mut prefix = vec![0u8; dir_end as usize];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut prefix)?;

    let mut extents: BTreeMap<u64, u64> = BTreeMap::new();
    let mut name_at: BTreeMap<u64, String> = BTreeMap::new();
    let mut hash_at: BTreeMap<u64, [u8; 16]> = BTreeMap::new();
    for e in &pack.entries {
        if e.size == 0 { continue; }
        if let Some(existing) = extents.insert(e.offset, e.size) {
            if existing != e.size { return Err(invalid("inconsistent aliased PCK extents")); }
        }
        name_at.entry(e.offset).or_insert_with(|| e.name.clone());
        hash_at.entry(e.offset).or_insert(e.hash);
    }

    let mut rewritten: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    let mut converted_names: Vec<Vec<u8>> = Vec::new();
    let mut mp3_total = 0u64;
    let mut mp3_converted = 0u64;
    let mut wav_total = 0u64;
    let mut wav_converted = 0u64;
    for (&off, &size) in &extents {
        let name = name_at.get(&off).map(String::as_str).unwrap_or("");
        let is_mp3 = !textures_only && name.ends_with(".mp3str");
        let is_wav = !textures_only && name.ends_with(".sample");
        let is_texture = textures_only && name.ends_with(".stex");
        if is_texture && protected.contains(&off) { continue; }
        if !is_mp3 && !is_wav && !is_texture { continue; }
        if size > MAX_TEX_ENTRY { continue; }
        if is_mp3 || is_texture { mp3_total += 1; } else { wav_total += 1; }
        let mut buf = vec![0u8; size as usize];
        file.seek(SeekFrom::Start(off))?; file.read_exact(&mut buf)?;
        let result = if is_texture {
            crate::gdst::transform_safe(&buf, profile)?
        } else if is_mp3 {
            crate::godot3::transform_mp3(&buf, quality)?
        } else {
            // Never use the failed IMA path. This export-only PCM path retains
            // the resource class and original container structure.
            crate::godot3::transform_sample_pcm(&buf, profile)?
        };
        if let Some(new_bytes) = result {
            // Only the MP3 path changes the resource type, so only it needs the
            // matching `.import` stub retyped.
            if is_mp3 {
                if let Some(entry) = pack.entries.iter().find(|e| e.offset == off) {
                    converted_names.push(entry.name_raw.clone());
                }
                mp3_converted += 1;
            } else if is_texture {
                mp3_converted += 1;
            } else {
                wav_converted += 1;
            }
            rewritten.insert(off, new_bytes);
        }
        super::cancelled()?;
    }
    if !converted_names.is_empty() {
        for (&off, &size) in &extents {
            let name = name_at.get(&off).map(String::as_str).unwrap_or("");
            if !name.ends_with(".import") || size > MAX_TEX_ENTRY { continue; }
            let mut buf = vec![0u8; size as usize];
            file.seek(SeekFrom::Start(off))?; file.read_exact(&mut buf)?;
            if let Some(patched) = crate::godot3::rewrite_import_type(&buf, &converted_names) {
                rewritten.insert(off, patched);
            }
            super::cancelled()?;
        }
    }

    // Gate before creating/writing an export. Low-yield candidates must not
    // consume SSD writes, and never remove a destination we did not create.
    let predicted_after = extents.iter().try_fold(dir_end, |sum, (off, size)| {
        sum.checked_add(rewritten.get(off).map_or(*size, |b| b.len() as u64))
            .ok_or_else(|| invalid("PCK output size overflow"))
    })?;
    let predicted_saved: i64 = extents.iter().map(|(off, size)| {
        *size as i64 - rewritten.get(off).map_or(*size, |b| b.len() as u64) as i64
    }).sum();
    let write_export = predicted_saved > 0 && predicted_saved as f64 * 100.0 / predicted_after.max(1) as f64 >= min_efficiency;
    let mut target_file = match output.filter(|_| write_export) {
        Some(path) => Some(fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).custom_flags(0x20000).open(path)?),
        None => None,
    };
    if let Some(t) = target_file.as_mut() { t.write_all(&prefix)?; }
    let mut new_offset: BTreeMap<u64, u64> = BTreeMap::new();
    let mut new_size: BTreeMap<u64, u64> = BTreeMap::new();
    let mut new_hash: BTreeMap<u64, [u8; 16]> = BTreeMap::new();
    let mut saved = 0i64;
    let mut pos = dir_end;
    for (&off, &size) in &extents {
        new_offset.insert(off, pos);
        match rewritten.get(&off) {
            Some(new_bytes) => {
                saved += size as i64 - new_bytes.len() as i64;
                new_size.insert(off, new_bytes.len() as u64);
                new_hash.insert(off, crate::md5::digest(new_bytes));
                if let Some(t) = target_file.as_mut() { t.write_all(new_bytes)?; }
                pos += new_bytes.len() as u64;
            }
            None => {
                new_size.insert(off, size);
                new_hash.insert(off, *hash_at.get(&off).unwrap_or(&[0; 16]));
                if let Some(t) = target_file.as_mut() { copy_range(&mut file, t, off, size)?; }
                pos += size;
            }
        }
        super::cancelled()?;
    }
    let after = pos;
    let efficiency = saved.max(0) as f64 * 100.0 / after.max(1) as f64;
    let keep = saved > 0 && efficiency >= min_efficiency;
    if let Some(path) = output {
        if keep {
            {
                let t = target_file.as_mut().ok_or_else(|| invalid("missing PCK writer"))?;
                for e in &pack.entries {
                    t.seek(SeekFrom::Start(e.offset_field))?;
                    t.write_all(&new_offset[&e.offset].to_le_bytes())?;
                    t.write_all(&new_size[&e.offset].to_le_bytes())?;
                    t.write_all(&new_hash[&e.offset])?;
                }
                t.sync_all()?;
            }
            godot3_verified(path, &pack, &new_offset, &new_size, &new_hash, &rewritten, &mut file, &meta, after)?;
        }
    }
    unchanged(&meta, &file.metadata()?)?;
    let status = if saved <= 0 { "NO_GAIN" } else if !keep { "LOW_EFFICIENCY" } else if output.is_some() { "EXPORTED" } else { "CANDIDATE" };
    if textures_only {
        println!("GODOT_TEXTURE|{}|{}|{}|{}|{}|{}|{:.4}|{}", meta.len(), after, pack.entries.len(), mp3_converted, mp3_total - mp3_converted, saved.max(0), efficiency, status);
    } else {
        println!("GODOT3|{}|{}|{}|{}|{}|{}|{}|{}|{:.4}|{}", meta.len(), after, pack.entries.len(), mp3_converted, mp3_total - mp3_converted, wav_converted, wav_total - wav_converted, saved.max(0), efficiency, status);
    }
    eprintln!("Source unchanged. Export mode checks generated payload bytes and untouched entries. Physical savings and in-game compatibility still need validation.");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn godot3_verified(
    output: &Path,
    original: &Pack,
    new_offset: &BTreeMap<u64, u64>,
    new_size: &BTreeMap<u64, u64>,
    new_hash: &BTreeMap<u64, [u8; 16]>,
    rewritten: &BTreeMap<u64, Vec<u8>>,
    source: &mut fs::File,
    source_meta: &fs::Metadata,
    after: u64,
) -> io::Result<()> {
    let (mut written, _) = open(output)?;
    let check = pck(&mut written, after)?;
    if check.version != original.version || check.entries.len() != original.entries.len() {
        return Err(invalid("exported Godot 3 PCK header verification failed"));
    }
    for (old, new) in original.entries.iter().zip(&check.entries) {
        if old.name != new.name || old.flags != new.flags { return Err(invalid("exported PCK directory changed")); }
        if new.offset != new_offset[&old.offset] || new.size != new_size[&old.offset] || new.hash != new_hash[&old.offset] {
            return Err(invalid("exported PCK entry metadata mismatch"));
        }
        if let Some(expected) = rewritten.get(&old.offset) {
            let mut actual = vec![0; expected.len()];
            written.seek(SeekFrom::Start(new.offset))?;
            written.read_exact(&mut actual)?;
            if &actual != expected || crate::md5::digest(&actual) != new.hash {
                return Err(invalid("exported PCK rewritten payload mismatch"));
            }
        } else if old.size > 0 {
            if !ranges_equal(source, old.offset, &mut written, new.offset, old.size)? {
                return Err(invalid("exported PCK untouched entry bytes differ"));
            }
        }
    }
    unchanged(source_meta, &source.metadata()?)?;
    Ok(())
}

/// Read-only verification that rewritten IMA-ADPCM `.sample` entries decode back
/// to the source PCM waveform. Reports the worst signal-to-noise ratio found.
pub fn godot3_verify(source: &Path, candidate: &Path) -> io::Result<()> {
    let (mut a, ma) = open(source)?;
    let (mut b, mb) = open(candidate)?;
    let pa = pck(&mut a, ma.len())?;
    let pb = pck(&mut b, mb.len())?;
    if pa.version != 1 || pb.version != 1 { return Err(invalid("godot3-verify expects PCK v1 files")); }
    let mut checked = 0u64;
    let mut worst = f64::MAX;
    let mut worst_name = String::new();
    let mut bytes = 0u64;
    let verbose = std::env::var("BGC_VERIFY_VERBOSE").is_ok();
    if pa.entries.len() != pb.entries.len() { return Err(invalid("candidate directory count differs")); }
    for (ea, eb) in pa.entries.iter().zip(&pb.entries) {
        if ea.name != eb.name { return Err(invalid("candidate directory names differ")); }
        if !ea.name.ends_with(".sample") { continue; }
        if ea.size > MAX_TEX_ENTRY || eb.size > MAX_TEX_ENTRY { continue; }
        let mut old = vec![0u8; ea.size as usize]; a.seek(SeekFrom::Start(ea.offset))?; a.read_exact(&mut old)?;
        let mut new = vec![0u8; eb.size as usize]; b.seek(SeekFrom::Start(eb.offset))?; b.read_exact(&mut new)?;
        if old == new { continue; }
        let old_res = crate::godot3::parse(&old)?;
        let new_res = crate::godot3::parse(&new)?;
        let old_format = match old_res.get("format") { Some(crate::godot3::Variant::Int(v)) => *v, _ => 1 };
        let new_format = match new_res.get("format") { Some(crate::godot3::Variant::Int(v)) => *v, _ => 1 };
        if old_format != 1 || new_format != 2 { return Err(invalid("unexpected changed sample format")); }
        if old_res.res_type != new_res.res_type { return Err(invalid("sample class changed")); }
        for (name, value) in &old_res.props {
            if name == "data" || name == "format" { continue; }
            if new_res.get(name) != Some(value) {
                return Err(invalid("sample metadata changed"));
            }
        }
        let stereo = matches!(old_res.get("stereo"), Some(crate::godot3::Variant::Bool(true)));
        let (Some(pcm_raw), Some(ima)) = (old_res.raw("data"), new_res.raw("data")) else { continue };
        let channels = if stereo { 2 } else { 1 };
        let frames = pcm_raw.len() / 2 / channels;
        if frames == 0 || pcm_raw.len() % (2 * channels) != 0 || ima.len() * 2 / channels != frames {
            return Err(invalid("sample duration/alignment changed"));
        }
        let pcm: Vec<i16> = pcm_raw.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        // PCM `data` is interleaved; compare channel by channel.
        let (l_orig, r_orig): (Vec<i16>, Vec<i16>) = if stereo {
            (0..frames).map(|f| pcm[f * 2]).collect::<Vec<_>>().into_iter()
                .zip((0..frames).map(|f| pcm[f * 2 + 1]))
                .unzip()
        } else {
            (pcm.clone(), pcm.clone())
        };
        let (l_dec, r_dec): (Vec<i16>, Vec<i16>) = if stereo {
            (crate::godot3::ima_decode(ima, frames, 2, 0),
             crate::godot3::ima_decode(ima, frames, 2, 1))
        } else {
            let d = crate::godot3::ima_decode(ima, frames, 1, 0);
            (d.clone(), d)
        };
        let (mut sig, mut noise) = (0.0f64, 0.0f64);
        if l_dec.len() != frames || r_dec.len() != frames { return Err(invalid("truncated decoded sample")); }
        for f in 0..frames {
            sig += (l_orig[f] as f64).powi(2) + (r_orig[f] as f64).powi(2);
            noise += (l_orig[f] as f64 - l_dec[f] as f64).powi(2)
                + (r_orig[f] as f64 - r_dec[f] as f64).powi(2);
        }
        let snr = if noise == 0.0 { 999.0 } else { 10.0 * (sig / noise).log10() };
        if verbose && (checked < 8 || snr < 15.0) {
            eprintln!(
                "  {} ch={} frames={} pcm={} ima={} expect_ima={} snr={:.2}",
                ea.name.rsplit('/').next().unwrap_or(&ea.name),
                channels, frames, pcm_raw.len(), ima.len(),
                frames / 2 * channels, snr
            );
        }
        if snr < worst { worst = snr; worst_name = ea.name.clone(); }
        bytes += ima.len() as u64;
        checked += 1;
        super::cancelled()?;
    }
    unchanged(&ma, &a.metadata()?)?;
    unchanged(&mb, &b.metadata()?)?;
    println!("GODOT3_VERIFY|{}|{}|{:.2}", checked, bytes, if worst.is_finite() { worst } else { 0.0 });
    if checked == 0 { return Err(invalid("no rewritten IMA-ADPCM samples found to verify")); }
    if worst < 20.0 {
        return Err(invalid(&format!("rewritten audio decodes with low fidelity (worst {worst:.2} dB in {worst_name})")));
    }
    eprintln!("Changed samples preserve frame counts and metadata and pass the 20 dB SNR gate; in-game validation is still required.");
    Ok(())
}

const PCK_ALIGNMENT: u64 = 32;
const MAX_TEX_ENTRY: u64 = 256 * 1024 * 1024;

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}
fn ranges_equal(left: &mut fs::File, lo: u64, right: &mut fs::File, ro: u64, size: u64) -> io::Result<bool> {
    let mut a = [0u8; 65536];
    let mut b = [0u8; 65536];
    let mut done = 0;
    while done < size {
        super::cancelled()?;
        let n = (size - done).min(a.len() as u64) as usize;
        left.seek(SeekFrom::Start(lo + done))?; left.read_exact(&mut a[..n])?;
        right.seek(SeekFrom::Start(ro + done))?; right.read_exact(&mut b[..n])?;
        if a[..n] != b[..n] { return Ok(false); }
        done += n as u64;
    }
    Ok(true)
}

/// Streaming, export-only Godot PCK texture rewrite. Supported `.ctex` (GST2)
/// textures are downscaled to the profile's longest edge and re-encoded in the
/// same pixel format; every other entry is copied byte for byte. The directory
/// is rebuilt with relocated offsets and recomputed MD5 hashes, then the output
/// is re-parsed and (for untouched entries) byte-compared against the source.
pub fn godot_texture_transform(input: &Path, output: Option<&Path>, min_efficiency: f64, profile: &str) -> io::Result<()> {
    if !min_efficiency.is_finite() || !(0.0..=100.0).contains(&min_efficiency) {
        return Err(invalid("Godot write-efficiency percentage must be between 0 and 100"));
    }
    let target = match crate::gst2::target_for(profile)? {
        Some(t) => t,
        None => {
            println!("GODOT_TEXTURE|native|0|0|0|0|0|0.0000|NO_PROFILE");
            eprintln!("Native/Lossless never rewrites textures; no output was written.");
            return Ok(());
        }
    };
    if let Some(path) = output {
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "PCK destination already exists")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (), Err(e) => return Err(e),
        }
    }
    let (mut file, meta) = open(input)?;
    file.seek(SeekFrom::Start(0))?;
    let _magic = u32le(&mut file)?;
    let version = u32le(&mut file)?;
    if version == 1 {
        return godot3_transform(input, output, min_efficiency, profile, true);
    }
    if !(3..=4).contains(&version) { return Err(invalid("PCK texture rewriting requires standalone Godot pack format v3/v4")); }
    let _engine = [u32le(&mut file)?, u32le(&mut file)?, u32le(&mut file)?];
    let flags = u32le(&mut file)?;
    if flags & !2 != 0 { return Err(invalid("encrypted/sparse/unknown Godot PCK layout unsupported")); }
    let base = u64le(&mut file)?;
    let directory = u64le(&mut file)?;
    // Rewriting relies on the v3 layout: structure, then payloads, then the
    // directory at the end. Anything else stays read-only.
    if directory <= base || directory >= meta.len() { return Err(invalid("unsupported PCK layout for rewriting")); }
    file.seek(SeekFrom::Start(0))?;
    let pack = pck(&mut file, meta.len())?;
    if pack.entries.iter().any(|e| e.flags != 0) { return Err(invalid("PCK texture rewriting requires plain entries without removal flags")); }
    let protected = protected_texture_offsets(&mut file, &pack)?;

    let mut prefix = vec![0u8; base as usize];
    file.seek(SeekFrom::Start(0))?; file.read_exact(&mut prefix)?;

    // Unique payload extents in file order; aliased entries provably share bytes.
    let mut extents: BTreeMap<u64, u64> = BTreeMap::new();
    let mut transformable: BTreeMap<u64, [u8; 16]> = BTreeMap::new();
    let mut original_hash: BTreeMap<u64, [u8; 16]> = BTreeMap::new();
    for e in &pack.entries {
        if e.size == 0 { continue; }
        if let Some(existing) = extents.insert(e.offset, e.size) {
            if existing != e.size { return Err(invalid("inconsistent aliased PCK extents")); }
        }
        original_hash.entry(e.offset).or_insert(e.hash);
        if crate::gst2::is_ctex_path(Path::new(&e.name)) { transformable.entry(e.offset).or_insert(e.hash); }
    }

    let mut new_offset: BTreeMap<u64, u64> = BTreeMap::new();
    let mut new_size: BTreeMap<u64, u64> = BTreeMap::new();
    let mut new_hash: BTreeMap<u64, [u8; 16]> = BTreeMap::new();
    let mut transformed = 0u64; let mut skipped = 0u64; let mut saved = 0i64;
    let mut rewritten: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
    let mut pos = base;
    for (&off, &size) in &extents {
        let aligned = align_up(pos, PCK_ALIGNMENT);
        let mut result_size = size;
        let mut result_hash = *original_hash.get(&off).unwrap_or(&[0; 16]);
        if transformable.contains_key(&off) && size <= MAX_TEX_ENTRY {
            if protected.contains(&off) {
                skipped += 1;
                new_offset.insert(off, aligned);
                new_size.insert(off, size);
                new_hash.insert(off, result_hash);
                pos = aligned + size;
                super::cancelled()?;
                continue;
            }
            let mut buf = vec![0u8; size as usize];
            file.seek(SeekFrom::Start(off))?; file.read_exact(&mut buf)?;
            match crate::gst2::transform_safe(&buf, &target)? {
                Some(new_bytes) => {
                    saved += size as i64 - new_bytes.len() as i64;
                    result_size = new_bytes.len() as u64;
                    result_hash = crate::md5::digest(&new_bytes);
                    transformed += 1;
                    rewritten.insert(off, new_bytes);
                }
                None => {
                    skipped += 1;
                }
            }
        }
        new_offset.insert(off, aligned);
        new_size.insert(off, result_size);
        new_hash.insert(off, result_hash);
        pos = aligned + result_size;
        super::cancelled()?;
    }

    let directory_offset = align_up(pos, PCK_ALIGNMENT);
    let mut directory_bytes = Vec::new();
    directory_bytes.extend_from_slice(&(pack.entries.len() as u32).to_le_bytes());
    for e in &pack.entries {
        directory_bytes.extend_from_slice(&(e.name_raw.len() as u32).to_le_bytes());
        directory_bytes.extend_from_slice(&e.name_raw);
        directory_bytes.extend_from_slice(&(new_offset[&e.offset] - base).to_le_bytes());
        directory_bytes.extend_from_slice(&new_size[&e.offset].to_le_bytes());
        directory_bytes.extend_from_slice(&new_hash[&e.offset]);
        directory_bytes.extend_from_slice(&e.flags.to_le_bytes());
    }
    let after = directory_offset + directory_bytes.len() as u64;
    let efficiency = saved.max(0) as f64 * 100.0 / after.max(1) as f64;
    let keep = saved > 0 && efficiency >= min_efficiency;

    if let Some(path) = output {
        if keep {
            // Create only after the savings gate passes; no speculative writes.
            let mut t = fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).custom_flags(0x20000).open(path)?;
            t.write_all(&prefix)?;
            let mut written_pos = base;
            for (&off, &size) in &extents {
                let aligned = new_offset[&off];
                if aligned > written_pos { t.write_all(&vec![0; (aligned - written_pos) as usize])?; }
                if let Some(bytes) = rewritten.get(&off) { t.write_all(bytes)?; }
                else { copy_range(&mut file, &mut t, off, size)?; }
                written_pos = aligned + new_size[&off];
            }
            if directory_offset > written_pos { t.write_all(&vec![0u8; (directory_offset - written_pos) as usize])?; }
            t.write_all(&directory_bytes)?;
            t.seek(SeekFrom::Start(32))?;
            t.write_all(&directory_offset.to_le_bytes())?;
            t.sync_all()?;
            changed_directory_verified(path, &pack, &new_offset, &new_size, &new_hash, &mut file, &meta, after)?;
        }
    }
    unchanged(&meta, &file.metadata()?)?;
    let status = if saved <= 0 { "NO_GAIN" } else if !keep { "LOW_EFFICIENCY" } else if output.is_some() { "EXPORTED" } else { "CANDIDATE" };
    println!("GODOT_TEXTURE|{}|{}|{}|{}|{}|{}|{:.4}|{}", meta.len(), after, pack.entries.len(), transformed, skipped, saved.max(0), efficiency, status);
    eprintln!("Source unchanged. Export checks payload hashes, directory metadata and unchanged entries; physical allocation and in-game loading still need validation.");
    Ok(())
}

/// In-place Godot PCK texture rewrite used by the main asset pipeline.
///
/// Unlike the export path this replaces `pack` itself. It writes the rebuilt
/// pack to a sibling temporary file, verifies it exactly like an export, then
/// recompresses and atomically renames it over the original. There is
/// deliberately no per-file backup: this tool targets Steam-managed games,
/// where "Verify integrity of game files" restores the original pack. A rewrite
/// that does not shrink the pack, or fails the write-efficiency gate, leaves the
/// original untouched. Returns the number of rewritten texture categories (0/1).
pub fn godot_texture_apply(pack: &Path, min_efficiency: f64, profile: &str, level: u8) -> io::Result<u64> {
    let tmp = sibling_temp(pack, "tex");
    let _ = fs::remove_file(&tmp);
    let mut written = false;
    let result = (|| -> io::Result<()> {
        godot_texture_transform(pack, Some(&tmp), min_efficiency, profile)?;
        // The transform writes nothing when the savings gate rejects the pack.
        match fs::symlink_metadata(&tmp) {
            Ok(_) => {
                let m = fs::metadata(pack)?;
                fs::set_permissions(&tmp, m.permissions())?;
                let f = fs::OpenOptions::new().read(true).open(&tmp)?;
                super::compress_file_best_effort(&f, pack, level)?;
                drop(f);
                File::open(tmp.parent().ok_or_else(|| invalid("invalid PCK path"))?)?.sync_all()?;
                fs::rename(&tmp, pack)?;
                File::open(pack.parent().ok_or_else(|| invalid("invalid PCK path"))?)?.sync_all()?;
                written = true;
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    Ok(written as u64)
}

fn sibling_temp(path: &Path, tag: &str) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".bgc-pck-{tag}-{}", std::process::id()));
    path.with_file_name(name)
}

#[allow(clippy::too_many_arguments)]
fn changed_directory_verified(
    output: &Path,
    original: &Pack,
    new_offset: &BTreeMap<u64, u64>,
    new_size: &BTreeMap<u64, u64>,
    new_hash: &BTreeMap<u64, [u8; 16]>,
    source: &mut fs::File,
    source_meta: &fs::Metadata,
    after: u64,
) -> io::Result<()> {
    let (mut written, _) = open(output)?;
    let check = pck(&mut written, after)?;
    if check.version != original.version || check.entries.len() != original.entries.len() {
        return Err(invalid("exported PCK header verification failed"));
    }
    for (old, new) in original.entries.iter().zip(&check.entries) {
        if old.name != new.name || old.flags != new.flags { return Err(invalid("exported PCK directory changed")); }
        if new.offset != new_offset[&old.offset] || new.size != new_size[&old.offset] || new.hash != new_hash[&old.offset] {
            return Err(invalid("exported PCK entry metadata mismatch"));
        }
        // Entries that were not rewritten must be byte-identical.
        if new.size == old.size && new.hash == old.hash && old.size > 0 {
            if !ranges_equal(source, old.offset, &mut written, new.offset, old.size)? {
                return Err(invalid("exported PCK untouched entry bytes differ"));
            }
        } else {
            let mut hash = crate::md5::Md5::new();
            written.seek(SeekFrom::Start(new.offset))?;
            let mut left = new.size;
            let mut buf = [0; 65536];
            while left > 0 {
                let n = left.min(buf.len() as u64) as usize;
                written.read_exact(&mut buf[..n])?;
                hash.update(&buf[..n]);
                left -= n as u64;
            }
            if hash.finalize() != new.hash { return Err(invalid("exported texture checksum differs")); }
        }
    }
    unchanged(source_meta, &source.metadata()?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(version: u32) -> Vec<u8> {
        let dir = if version == 1 { 84 } else { 96 };
        let mut b = Vec::new(); b.extend_from_slice(b"GDPC"); b.extend_from_slice(&version.to_le_bytes());
        for n in [4u32, 5, 1] { b.extend_from_slice(&n.to_le_bytes()); }
        if version >= 2 { b.extend_from_slice(&0u32.to_le_bytes()); b.extend_from_slice(&256u64.to_le_bytes()); }
        if version >= 3 { b.extend_from_slice(&(dir as u64).to_le_bytes()); }
        b.resize(dir, 0); b.extend_from_slice(&1u32.to_le_bytes());
        let name = b"res://x.ctex\0"; b.extend_from_slice(&(name.len() as u32).to_le_bytes()); b.extend_from_slice(name);
        b.extend_from_slice(&(if version == 1 { 256u64 } else { 0 }).to_le_bytes());
        b.extend_from_slice(&52u64.to_le_bytes()); b.extend_from_slice(&[0; 16]);
        if version >= 2 { b.extend_from_slice(&0u32.to_le_bytes()); }
        b.resize(308, 0); b
    }
    #[test]
    fn pck_versions_and_truncation() {
        for v in 1..=4 {
            let b = fixture(v); let p = pck(&mut io::Cursor::new(&b), b.len() as u64).unwrap();
            assert_eq!(p.entries[0].offset, 256); assert_eq!(p.payload, 52);
            assert!(pck(&mut io::Cursor::new(&b[..100]), 100).is_err());
        }
    }
    #[test]
    fn godot_texture_formats_are_named_without_claiming_decoders() {
        assert_eq!(godot_image_format(19), "DXT5_BC3");
        assert_eq!(godot_image_format(22), "BPTC_RGBA_BC7");
        assert_eq!(godot_image_format(99), "unknown");
    }
    #[test]
    fn rsrc_class_header_is_bounded_and_fail_closed() {
        let mut b = b"RSRC".to_vec();
        for n in [0u32, 0, 4, 3, 6, 15] { b.extend_from_slice(&n.to_le_bytes()); }
        b.extend_from_slice(b"AudioStreamWAV\0");
        assert_eq!(rsrc_class(&mut io::Cursor::new(&b), 0, b.len() as u64).unwrap().as_deref(), Some("AudioStreamWAV"));
        assert_eq!(rsrc_class(&mut io::Cursor::new(&b), 0, 30).unwrap(), None);
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(rsrc_class(&mut io::Cursor::new(&b), 0, b.len() as u64).unwrap(), None);
    }
    fn godot_wav_fixture() -> Vec<u8> {
        fn u32v(b: &mut Vec<u8>, v: u32) { b.extend_from_slice(&v.to_le_bytes()); }
        fn u64v(b: &mut Vec<u8>, v: u64) { b.extend_from_slice(&v.to_le_bytes()); }
        fn string(b: &mut Vec<u8>, s: &str) { u32v(b, s.len() as u32 + 1); b.extend_from_slice(s.as_bytes()); b.push(0); }
        let mut b = b"RSRC".to_vec();
        for v in [0, 0, 4, 3, 6] { u32v(&mut b, v); }
        string(&mut b, "AudioStreamWAV"); u64v(&mut b, 0); u32v(&mut b, 3); u64v(&mut b, u64::MAX);
        for _ in 0..11 { u32v(&mut b, 0); }
        let names = ["data", "format", "loop_mode", "loop_begin", "loop_end", "mix_rate", "stereo", "script"];
        u32v(&mut b, names.len() as u32); for name in names { string(&mut b, name); }
        u32v(&mut b, 0); // external resources
        u32v(&mut b, 1); string(&mut b, "local://AudioStreamWAV_fixture");
        let offset_field = b.len(); u64v(&mut b, 0);
        let resource_offset = b.len() as u64;
        string(&mut b, "AudioStreamWAV"); u32v(&mut b, 8);
        // Packed byte array with 16-bit PCM data.
        u32v(&mut b, 0); u32v(&mut b, 31); u32v(&mut b, 4); b.extend_from_slice(&[1, 2, 3, 4]);
        for (key, tag, value) in [(1, 3, 1), (2, 3, 1), (3, 3, 0), (4, 3, 4), (5, 3, 48000), (6, 2, 1)] {
            u32v(&mut b, key); u32v(&mut b, tag); u32v(&mut b, value);
        }
        u32v(&mut b, 7); u32v(&mut b, 1); // nil script property
        b[offset_field..offset_field + 8].copy_from_slice(&resource_offset.to_le_bytes());
        b.extend_from_slice(b"RSRC");
        b
    }
    #[test]
    fn godot_wav_resource_extracts_codec_payload_and_loop_metadata() {
        let b = godot_wav_fixture();
        let info = wav_resource(&mut io::Cursor::new(&b), 0, b.len() as u64).unwrap().unwrap();
        assert_eq!(info.format, 1); assert!(info.stereo); assert_eq!(info.rate, 48000);
        assert_eq!(info.loop_mode, 1); assert_eq!(info.loop_begin, 0); assert_eq!(info.loop_end, 4);
        assert_eq!(info.data_bytes, 4);
        // Truncation must fail closed instead of reporting a partial summary.
        assert!(wav_resource(&mut io::Cursor::new(&b[..b.len() - 8]), 0, (b.len() - 8) as u64).is_err());
    }
    #[test]
    fn refuses_encryption_unknown_flags_and_overflow() {
        let mut b = fixture(2); b[20..24].copy_from_slice(&1u32.to_le_bytes());
        assert!(pck(&mut io::Cursor::new(&b), b.len() as u64).is_err());
        b[20..24].copy_from_slice(&8u32.to_le_bytes()); assert!(pck(&mut io::Cursor::new(&b), b.len() as u64).is_err());
        b[20..24].copy_from_slice(&0u32.to_le_bytes()); b[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(pck(&mut io::Cursor::new(&b), b.len() as u64).is_err());
    }
    #[test]
    fn footer_declarations_encryption_and_bad_index() {
        for version in [3u32, 7, 8, 9, 10, 11] {
            let prefix = if version >= 7 { 17 } else { 0 };
            let mut b = vec![0; prefix]; if prefix > 0 { b[prefix - 1] = 1; }
            b.extend_from_slice(&0x5a6f12e1u32.to_le_bytes()); b.extend_from_slice(&version.to_le_bytes());
            b.extend_from_slice(&100u64.to_le_bytes()); b.extend_from_slice(&30u64.to_le_bytes()); b.extend_from_slice(&[0; 20]);
            if version == 9 { b.push(0); }
            if version >= 8 { let p = b.len(); b.resize(p + 160, 0); b[p..p + 4].copy_from_slice(b"Zlib"); }
            let f = footer(&b, 1000).unwrap(); assert_eq!(f.version, version); assert_eq!(f.encrypted, prefix > 0);
            b[prefix + 8..prefix + 16].copy_from_slice(&u64::MAX.to_le_bytes()); assert!(footer(&b, 1000).is_err());
        }
        assert!(footer(b"not a pak", 9).is_err());
    }
    #[test]
    fn unreal_primary_hash_verified_and_corruption_rejected_without_codec_guessing() {
        use sha1::{Digest, Sha1};
        let bytes = b"prefixprimary index bytestrailer";
        let index = b"primary index bytes";
        let hash = Sha1::digest(index).into();
        let f = Footer { version: 11, offset: 6, size: index.len() as u64, hash,
            encrypted: false, frozen: false, codecs: vec!["Oodle".into()] };
        assert_eq!(verify_unreal_index(&mut io::Cursor::new(bytes), &f).unwrap(), "VERIFIED_PRIMARY_SHA1");
        let mut broken = bytes.to_vec(); broken[7] ^= 1;
        assert!(verify_unreal_index(&mut io::Cursor::new(broken), &f).is_err());
        assert!(verify_unreal_index(&mut io::Cursor::new(&bytes[..10]), &f).is_err());
        let encrypted = Footer { encrypted: true, ..f };
        assert_eq!(verify_unreal_index(&mut io::Cursor::new([]), &encrypted).unwrap(), "SKIPPED_ENCRYPTED");
        let oversized = Footer { encrypted: false, size: 256 * 1024 * 1024 + 1, ..encrypted };
        assert_eq!(verify_unreal_index(&mut io::Cursor::new([]), &oversized).unwrap(), "SKIPPED_BUDGET");
    }
    fn duplicate_fixture(version: u32) -> Vec<u8> {
        let directory = if version == 3 { 1280 } else if version == 1 { 84 } else { 96 };
        let mut b = b"GDPC".to_vec(); b.extend_from_slice(&version.to_le_bytes());
        for n in [4u32, 5, 1] { b.extend_from_slice(&n.to_le_bytes()); }
        if version >= 2 { b.extend_from_slice(&0u32.to_le_bytes()); b.extend_from_slice(&512u64.to_le_bytes()); }
        if version == 3 { b.extend_from_slice(&(directory as u64).to_le_bytes()); }
        b.resize(directory, 0); b.extend_from_slice(&4u32.to_le_bytes());
        for (i, off) in [512u64, 768, 1024, 768].into_iter().enumerate() {
            let name = format!("res://{i}.bin"); b.extend_from_slice(&(name.len() as u32).to_le_bytes()); b.extend_from_slice(name.as_bytes());
            let relative = if version == 1 { off } else { off - 512 };
            b.extend_from_slice(&relative.to_le_bytes()); b.extend_from_slice(&256u64.to_le_bytes());
            b.extend_from_slice(&[7; 16]); // Deliberately same hash for unequal bytes too.
            if version >= 2 { b.extend_from_slice(&0u32.to_le_bytes()); }
        }
        b.resize(b.len().max(1280), 0);
        b[512..1024].fill(42); b[1024..1280].fill(99); b
    }
    #[test]
    fn dedup_verifies_hash_collisions_and_relocates_all_versions() {
        let root = std::env::temp_dir().join(format!("bgc-pck-dedup-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        for version in 1..=3 {
            let b = duplicate_fixture(version); let pack = pck(&mut io::Cursor::new(&b), b.len() as u64).unwrap();
            let plan = dedup_plan(&mut io::Cursor::new(&b), &pack).unwrap();
            assert_eq!(plan.saved, 256); assert_eq!(plan.aliases.get(&768), Some(&512));
            assert!(!plan.aliases.contains_key(&1024)); // Hash alone is not enough.
            let input = root.join(format!("in-{version}")); let output = root.join(format!("out-{version}"));
            fs::write(&input, &b).unwrap(); godot_dedup(&input, Some(&output), 0.0).unwrap();
            let result = fs::read(&output).unwrap(); assert_eq!(result.len(), b.len() - 256);
            let check = pck(&mut io::Cursor::new(&result), result.len() as u64).unwrap();
            assert_eq!(check.entries[0].offset, check.entries[1].offset);
            assert_eq!(check.entries[1].offset, check.entries[3].offset);
            assert_ne!(check.entries[2].offset, check.entries[1].offset);
            assert!(godot_dedup(&input, Some(&output), 0.0).is_err());
            assert!(godot_dedup(&input, Some(&input), 0.0).is_err());
            let skipped = root.join(format!("skip-{version}")); godot_dedup(&input, Some(&skipped), 100.0).unwrap();
            assert!(!skipped.exists()); assert_eq!(fs::read(&input).unwrap(), b);
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn overlapping_and_encrypted_entries_cannot_be_deduplicated() {
        let b = duplicate_fixture(2); let mut pack = pck(&mut io::Cursor::new(&b), b.len() as u64).unwrap();
        pack.entries[2].offset = 700; assert!(dedup_plan(&mut io::Cursor::new(&b), &pack).is_err());
        let mut pack = pck(&mut io::Cursor::new(&b), b.len() as u64).unwrap();
        pack.entries[1].flags = 1; pack.entries[3].flags = 1;
        let plan = dedup_plan(&mut io::Cursor::new(&b), &pack).unwrap(); assert_eq!(plan.saved, 0);
    }

    // A plain v3 pack with the same layout the exporter assumes: header, then
    // 32-byte-aligned payloads, then the directory at the very end.
    fn build_pck(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut b = b"GDPC".to_vec();
        b.extend_from_slice(&3u32.to_le_bytes());
        for n in [4u32, 5, 1] { b.extend_from_slice(&n.to_le_bytes()); }
        b.extend_from_slice(&2u32.to_le_bytes()); // REL_FILEBASE
        let base_field = b.len(); b.extend_from_slice(&0u64.to_le_bytes());
        let dir_field = b.len(); b.extend_from_slice(&0u64.to_le_bytes());
        for _ in 0..16 { b.extend_from_slice(&0u32.to_le_bytes()); }
        while b.len() % 32 != 0 { b.push(0); }
        let base = b.len();
        let mut slots = Vec::new();
        for (name, data) in entries {
            while b.len() % 32 != 0 { b.push(0); }
            slots.push((name.to_string(), b.len() - base, data.len(), crate::md5::digest(data)));
            b.extend_from_slice(data);
        }
        while b.len() % 32 != 0 { b.push(0); }
        let directory = b.len();
        b[base_field..base_field + 8].copy_from_slice(&(base as u64).to_le_bytes());
        b[dir_field..dir_field + 8].copy_from_slice(&(directory as u64).to_le_bytes());
        b.extend_from_slice(&(slots.len() as u32).to_le_bytes());
        for (name, rel, size, hash) in slots {
            let raw = format!("{name}\0");
            b.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            b.extend_from_slice(raw.as_bytes());
            b.extend_from_slice(&(rel as u64).to_le_bytes());
            b.extend_from_slice(&(size as u64).to_le_bytes());
            b.extend_from_slice(&hash);
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        b
    }
    fn raw_ctex(width: u32, height: u32) -> Vec<u8> {
        let mut out = b"GST2".to_vec();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&[0u8; 20]); // data-format flags, mipmap limit, reserved
        out.extend_from_slice(&0u32.to_le_bytes()); // encoding = raw image
        out.extend_from_slice(&(width as u16).to_le_bytes());
        out.extend_from_slice(&(height as u16).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // no mipmaps
        out.extend_from_slice(&5u32.to_le_bytes()); // RGBA8
        for i in 0..(width * height) {
            out.extend_from_slice(&[(i % 251) as u8, (i % 253) as u8, (i % 255) as u8, 255]);
        }
        out
    }
    #[test]
    fn atlas_metadata_and_names_protect_payloads_in_every_profile() {
        let root = std::env::temp_dir().join(format!("bgc-atlas-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let entries = [
            ("res://.godot/sheet.ctex", raw_ctex(1200, 900)),
            ("res://card_atlas_0.ctex", raw_ctex(1200, 900)),
            ("res://small.ctex", raw_ctex(256, 256)),
            ("res://background.ctex", raw_ctex(1200, 900)),
            ("res://card.tres", b"[gd_resource type=\"AtlasTexture\"]\n[ext_resource path=\"res://art/sheet.png\"]\nregion = Rect2(1, 1, 250, 351)\n".to_vec()),
            ("res://art/sheet.png.import", b"[remap]\npath=\"res://.godot/sheet.ctex\"\n".to_vec()),
        ];
        let input = root.join("in.pck");
        fs::write(&input, build_pck(&entries)).unwrap();
        for profile in ["ultra-performance", "performance", "balanced", "quality", "ultra-quality"] {
            let (mut f, m) = open(&input).unwrap();
            let pack = pck(&mut f, m.len()).unwrap();
            let protected = protected_texture_offsets(&mut f, &pack).unwrap();
            assert!(protected.contains(&pack.entries[0].offset));
            assert!(protected.contains(&pack.entries[1].offset));
            assert!(!protected.contains(&pack.entries[3].offset));
            let output = root.join(format!("{profile}.pck"));
            godot_texture_transform(&input, Some(&output), 0.0, profile).unwrap();
            if output.exists() {
                let bytes = fs::read(output).unwrap();
                let check = pck(&mut io::Cursor::new(&bytes), bytes.len() as u64).unwrap();
                for i in [0usize, 1, 2, 4, 5] {
                    let e = &check.entries[i];
                    assert_eq!(&bytes[e.offset as usize..(e.offset + e.size) as usize], &entries[i].1, "{profile}: protected resource {i}");
                }
                assert!(check.entries[3].size < pack.entries[3].size);
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resource_path_inspection_handles_text_and_binary_delimiters() {
        let paths = resource_paths(b"path=\"res://art/a.png\"\nres://.godot/b.ctex\0padding");
        assert_eq!(paths, BTreeSet::from(["art/a.png".to_owned(), ".godot/b.ctex".to_owned()]));
    }
    #[test]
    fn godot3_texture_export_preserves_payloads_and_gates_before_writing() {
        let root = std::env::temp_dir().join(format!("bgc-gdst-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let texture = crate::gdst::tests::fixture(600, 600);
        let entries = [("res://art.stex", texture), ("res://data.bin", vec![7u8; 1000])];
        let mut bytes = b"GDPC".to_vec();
        for v in [1u32, 3, 7, 0] { bytes.extend_from_slice(&v.to_le_bytes()); }
        bytes.resize(84, 0);
        bytes.extend_from_slice(&2u32.to_le_bytes());
        let mut offset = 88 + entries.iter().map(|(name, _)| 4 + name.len() + 1 + 32).sum::<usize>();
        for (name, payload) in &entries {
            bytes.extend_from_slice(&(name.len() as u32 + 1).to_le_bytes());
            bytes.extend_from_slice(name.as_bytes()); bytes.push(0);
            bytes.extend_from_slice(&(offset as u64).to_le_bytes());
            bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&crate::md5::digest(payload));
            offset += payload.len();
        }
        for (_, payload) in &entries { bytes.extend_from_slice(payload); }
        let input = root.join("in.pck"); let output = root.join("out.pck");
        fs::write(&input, &bytes).unwrap();
        godot_texture_transform(&input, Some(&output), 0.0, "ultra-performance").unwrap();
        let result = fs::read(&output).unwrap();
        let pack = pck(&mut io::Cursor::new(&result), result.len() as u64).unwrap();
        assert_eq!(pack.version, 1);
        let e = &pack.entries[1];
        assert_eq!(&result[e.offset as usize..(e.offset + e.size) as usize], &entries[1].1);
        assert!(result.len() < bytes.len());
        assert!(godot_texture_transform(&input, Some(&output), 0.0, "ultra-performance").is_err());
        let rejected = root.join("rejected.pck");
        godot_texture_transform(&input, Some(&rejected), 100.0, "ultra-performance").unwrap();
        assert!(!rejected.exists());
        assert_eq!(fs::read(&input).unwrap(), bytes);
        fs::remove_file(input).unwrap(); fs::remove_file(output).unwrap(); fs::remove_dir(root).unwrap();
    }
    #[test]
    fn texture_export_shrinks_ctex_and_preserves_every_other_entry() {
        let root = std::env::temp_dir().join(format!("bgc-pck-tex-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let ctex = raw_ctex(1200, 900);
        let binary: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let pack_bytes = build_pck(&[("res://art.ctex", ctex.clone()), ("res://data.bin", binary.clone())]);
        let input = root.join("in.pck"); let output = root.join("out.pck");
        fs::write(&input, &pack_bytes).unwrap();
        godot_texture_transform(&input, Some(&output), 0.0, "ultra-performance").unwrap();
        assert_eq!(fs::read(&input).unwrap(), pack_bytes, "source must be untouched");
        let result = fs::read(&output).unwrap();
        assert!(result.len() < pack_bytes.len(), "export must shrink");
        let pack = pck(&mut io::Cursor::new(&result), result.len() as u64).unwrap();
        assert_eq!(pack.entries.len(), 2);
        let after = &result[pack.entries[0].offset as usize..(pack.entries[0].offset + pack.entries[0].size) as usize];
        assert_eq!(crate::md5::digest(after), pack.entries[0].hash, "hash must match the rewritten payload");
        assert!(crate::gst2::inspect(after).unwrap().width.max(crate::gst2::inspect(after).unwrap().height) <= 640);
        let untouched = &result[pack.entries[1].offset as usize..(pack.entries[1].offset + pack.entries[1].size) as usize];
        assert_eq!(untouched, &binary[..], "non-ctex entry must be byte-identical");
        assert_eq!(pack.entries[1].hash, crate::md5::digest(&binary), "unchanged entry keeps its real MD5");
        // Refuses to overwrite and refuses a wasteful run without writing.
        assert!(godot_texture_transform(&input, Some(&output), 0.0, "ultra-performance").is_err());
        let rejected = root.join("rejected.pck");
        godot_texture_transform(&input, Some(&rejected), 100.0, "balanced").unwrap();
        assert!(!rejected.exists());
        let native = root.join("native.pck");
        godot_texture_transform(&input, Some(&native), 0.0, "native").unwrap();
        assert!(!native.exists());
        assert_eq!(fs::read(&input).unwrap(), pack_bytes);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plain_v4_texture_export_preserves_pack_version() {
        let root = std::env::temp_dir().join(format!("bgc-v4-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let mut bytes = build_pck(&[("res://art.ctex", raw_ctex(1200, 900))]);
        bytes[4..8].copy_from_slice(&4u32.to_le_bytes());
        let input = root.join("in.pck"); let output = root.join("out.pck");
        fs::write(&input, &bytes).unwrap();
        godot_texture_transform(&input, Some(&output), 0.0, "ultra-performance").unwrap();
        let result = fs::read(&output).unwrap();
        let pack = pck(&mut io::Cursor::new(&result), result.len() as u64).unwrap();
        assert_eq!(pack.version, 4);
        assert!(result.len() < bytes.len());
        assert_eq!(fs::read(&input).unwrap(), bytes);
        fs::remove_file(input).unwrap(); fs::remove_file(output).unwrap(); fs::remove_dir(root).unwrap();
    }

    // Godot 3 in-place apply must run both transforms in one pass: the audio
    // pass rewrites `.sample`, then the texture pass reads that result and
    // rewrites `.stex`. Nothing is backed up; recovery is Steam verify.
    #[test]
    fn godot3_apply_chains_audio_and_texture_and_is_idempotent() {
        let root = std::env::temp_dir().join(format!("bgc-g3-apply-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&root).unwrap();
        let frames = 4800usize;
        let mut pcm = Vec::new();
        for i in 0..frames {
            let v = ((i as f64 * std::f64::consts::TAU * 440.0 / 48000.0).sin() * 16000.0) as i16;
            pcm.extend_from_slice(&v.to_le_bytes());
            pcm.extend_from_slice(&(-v).to_le_bytes());
        }
        let audio = crate::godot3::write_resource("AudioStreamSample", &[
            ("data", crate::godot3::Variant::Raw(pcm)), ("format", crate::godot3::Variant::Int(1)),
            ("mix_rate", crate::godot3::Variant::Int(48000)), ("stereo", crate::godot3::Variant::Bool(true)),
        ]).unwrap();
        let entries = [("res://tone.sample", audio), ("res://art.stex", crate::gdst::tests::fixture(1000, 600))];
        // Plain v1 pack: 84-byte header, directory immediately after it.
        let mut bytes = b"GDPC".to_vec();
        for v in [1u32, 3, 7, 0] { bytes.extend_from_slice(&v.to_le_bytes()); }
        bytes.resize(84, 0);
        bytes.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        let mut offset = 88 + entries.iter().map(|(name, _)| 4 + name.len() + 1 + 32).sum::<usize>();
        for (name, payload) in &entries {
            bytes.extend_from_slice(&(name.len() as u32 + 1).to_le_bytes());
            bytes.extend_from_slice(name.as_bytes()); bytes.push(0);
            bytes.extend_from_slice(&(offset as u64).to_le_bytes());
            bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            bytes.extend_from_slice(&crate::md5::digest(payload));
            offset += payload.len();
        }
        for (_, payload) in &entries { bytes.extend_from_slice(payload); }
        let pack = root.join("game.pck");
        fs::write(&pack, &bytes).unwrap();

        let (textures, audio) = godot3_apply(&pack, 0.0, "ultra-performance", 1).unwrap();
        assert_eq!((textures, audio), (1, 1), "both categories must be rewritten");
        let rewritten = fs::read(&pack).unwrap();
        assert!(rewritten.len() < bytes.len());
        let check = pck(&mut io::Cursor::new(&rewritten), rewritten.len() as u64).unwrap();
        assert_eq!(check.entries.len(), 2);
        for e in &check.entries {
            let payload = &rewritten[e.offset as usize..(e.offset + e.size) as usize];
            assert_eq!(crate::md5::digest(payload), e.hash, "directory hash must match payload");
        }
        // No sibling temporaries are left behind and no backup tree appears.
        for entry in fs::read_dir(&root).unwrap() {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            assert!(!name.contains("bgc-pck"), "temp file left behind: {name}");
        }
        assert!(!root.join(".bgc-assets-backup").exists());

        // Nothing left to gain: byte-for-byte identical and reported as no-op.
        let (textures, audio) = godot3_apply(&pack, 0.0, "ultra-performance", 1).unwrap();
        assert_eq!((textures, audio), (0, 0));
        assert_eq!(fs::read(&pack).unwrap(), rewritten);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn godot4_texture_apply_rewrites_in_place_and_is_idempotent() {
        for version in [3u32, 4] {
            let root = std::env::temp_dir().join(format!("bgc-g4-apply-{version}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir(&root).unwrap();
            let binary: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
            let mut bytes = build_pck(&[("res://art.ctex", raw_ctex(1200, 900)), ("res://data.bin", binary.clone())]);
            bytes[4..8].copy_from_slice(&version.to_le_bytes());
            let pack = root.join("game.pck");
            fs::write(&pack, &bytes).unwrap();

            let textures = godot_texture_apply(&pack, 0.0, "ultra-performance", 1).unwrap();
            assert_eq!(textures, 1, "v{version} texture must be rewritten");
            let rewritten = fs::read(&pack).unwrap();
            assert!(rewritten.len() < bytes.len());
            let check = pck(&mut io::Cursor::new(&rewritten), rewritten.len() as u64).unwrap();
            assert_eq!(check.version, version);
            assert_eq!(check.entries.len(), 2);
            let ctex = &rewritten[check.entries[0].offset as usize..(check.entries[0].offset + check.entries[0].size) as usize];
            let info = crate::gst2::inspect(ctex).unwrap();
            assert!(info.width.max(info.height) <= 640);
            let untouched = &rewritten[check.entries[1].offset as usize..(check.entries[1].offset + check.entries[1].size) as usize];
            assert_eq!(untouched, &binary[..], "non-ctex entry must be byte-identical");
            for entry in fs::read_dir(&root).unwrap() {
                let name = entry.unwrap().file_name();
                let name = name.to_string_lossy();
                assert!(!name.contains("bgc-pck"), "temp file left behind: {name}");
            }

            let textures = godot_texture_apply(&pack, 0.0, "ultra-performance", 1).unwrap();
            assert_eq!(textures, 0);
            assert_eq!(fs::read(&pack).unwrap(), rewritten);
            fs::remove_dir_all(root).unwrap();
        }
    }
}
