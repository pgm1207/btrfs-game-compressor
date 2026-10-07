//! Development-only standalone Godot 4.3 binary AudioStreamWAV metadata audit.
//! RSCC remains opaque. No PCK traversal, decompression, object instantiation,
//! external-reference loading, audio decoding or writer.
//! Format/variant tags are from Godot's resource_format_binary, not Variant IDs.
use std::{collections::{BTreeMap, BTreeSet}, io, path::Path};
use super::{audit_io::{field, Source}, invalid};

const MAX_READ: u64 = 1024 * 1024;
const MAX_STRINGS: u32 = 4096;
const MAX_REFS: u32 = 1024;
const MAX_PROPERTIES: u32 = 128;

struct Reader<'a> { source: &'a mut Source, offset: u64, end: u64 }
impl Reader<'_> {
    fn take(&mut self, size: usize) -> io::Result<Vec<u8>> {
        let end = self.offset.checked_add(size as u64).ok_or_else(|| invalid("Godot resource offset overflow"))?;
        if end > self.end { return Err(invalid("Godot resource field exceeds its extent")); }
        let bytes = self.source.read_at(self.offset, size)?;
        self.offset = end;
        Ok(bytes)
    }
    fn skip(&mut self, size: u64) -> io::Result<()> {
        let end = self.offset.checked_add(size).ok_or_else(|| invalid("Godot resource offset overflow"))?;
        if end > self.end { return Err(invalid("Godot resource blob exceeds its extent")); }
        self.source.range(self.offset, size)?;
        self.offset = end;
        Ok(())
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.as_slice().try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.as_slice().try_into().unwrap()))
    }
    fn text_len(&mut self, len: u32) -> io::Result<String> {
        if len > 4096 { return Err(invalid("Godot resource text exceeds audit limit")); }
        if len == 0 { return Ok(String::new()); }
        let bytes = self.take(len as usize)?;
        if bytes.last() != Some(&0) || bytes[..bytes.len() - 1].contains(&0) {
            return Err(invalid("invalid Godot resource string termination"));
        }
        std::str::from_utf8(&bytes[..bytes.len() - 1])
            .map(str::to_owned).map_err(|_| invalid("invalid Godot resource UTF-8"))
    }
    fn text(&mut self) -> io::Result<String> { let len = self.u32()?; self.text_len(len) }
    fn property_name(&mut self, strings: &[String]) -> io::Result<String> {
        let index = self.u32()?;
        if index & 0x80000000 != 0 { self.text_len(index & 0x7fffffff) }
        else { strings.get(index as usize).cloned().ok_or_else(|| invalid("Godot property string index out of bounds")) }
    }
}

struct External { kind: String, path: String, uid: Option<u64> }
struct Internal { path: String, offset: u64 }
enum Value { Number(i64), Bool(bool), Text(String), Bytes(u64, u64), Empty }
struct Audio {
    format: i64, rate: i64, stereo: bool, loop_mode: i64,
    loop_begin: i64, loop_end: i64, data_offset: u64, data_bytes: u64,
    frames: Option<u64>,
}
struct Inventory {
    major: u32, minor: u32, format: u32, flags: u32, uid: u64, kind: String,
    strings: Vec<String>, external: Vec<External>, internal: Vec<Internal>,
    properties: BTreeMap<String, Value>, audio: Option<Audio>, status: &'static str,
}

fn number(properties: &BTreeMap<String, Value>, name: &str, default: i64) -> io::Result<i64> {
    match properties.get(name) {
        Some(Value::Number(n)) => Ok(*n), None => Ok(default),
        _ => Err(invalid("Godot audio numeric property has the wrong variant type")),
    }
}
fn boolean(properties: &BTreeMap<String, Value>, name: &str, default: bool) -> io::Result<bool> {
    match properties.get(name) {
        Some(Value::Bool(b)) => Ok(*b), None => Ok(default),
        _ => Err(invalid("Godot audio boolean property has the wrong variant type")),
    }
}

fn inspect(source: &mut Source) -> io::Result<Inventory> {
    let mut v = Inventory { major: 0, minor: 0, format: 0, flags: 0, uid: 0, kind: String::new(),
        strings: Vec::new(), external: Vec::new(), internal: Vec::new(), properties: BTreeMap::new(),
        audio: None, status: "METADATA_ONLY" };
    let length = source.len();
    let mut r = Reader { source, offset: 0, end: length };
    let magic = r.take(4)?;
    if magic == b"RSCC" {
        v.status = "COMPRESSED_RESOURCE_NOT_DECODED";
        return Ok(v);
    }
    if magic != b"RSRC" { return Err(invalid("not a Godot binary resource")); }
    let big = r.u32()?;
    let real64 = r.u32()?;
    if big > 1 || real64 > 1 { return Err(invalid("invalid Godot resource endian/real-width flag")); }
    if big != 0 { v.status = "BIG_ENDIAN_RESOURCE_NOT_PARSED"; return Ok(v); }
    v.major = r.u32()?; v.minor = r.u32()?; v.format = r.u32()?;
    if v.major != 4 || v.minor != 3 || v.format != 6 {
        v.status = "UNSUPPORTED_ENGINE_OR_RESOURCE_VERSION";
        return Ok(v);
    }
    v.kind = r.text()?;
    let import_offset = r.u64()?;
    v.flags = r.u32()?;
    v.uid = r.u64()?;
    if v.flags & !15 != 0 { v.status = "UNKNOWN_HEADER_FLAGS"; return Ok(v); }
    if v.flags & 8 != 0 {
        r.text()?;
        v.status = "SCRIPT_CLASS_RESOURCE_NOT_PARSED";
        return Ok(v);
    }
    for _ in 0..11 {
        if r.u32()? != 0 { v.status = "UNKNOWN_RESERVED_HEADER_FIELDS"; return Ok(v); }
    }
    if import_offset != 0 { v.status = "IMPORT_METADATA_LAYOUT_NOT_PARSED"; return Ok(v); }
    let count = r.u32()?;
    if count > MAX_STRINGS { return Err(invalid("Godot resource string table exceeds audit limit")); }
    for _ in 0..count { v.strings.push(r.text()?); }
    let count = r.u32()?;
    if count > MAX_REFS { return Err(invalid("Godot external-reference count exceeds audit limit")); }
    for _ in 0..count {
        let kind = r.text()?; let path = r.text()?;
        let uid = if v.flags & 2 != 0 { Some(r.u64()?) } else { None };
        v.external.push(External { kind, path, uid });
    }
    let count = r.u32()?;
    if count == 0 || count > MAX_REFS { return Err(invalid("Godot internal-resource count exceeds audit subset")); }
    let mut offsets = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for _ in 0..count {
        let path = r.text()?; let offset = r.u64()?;
        if !offsets.insert(offset) || !paths.insert(path.clone()) {
            return Err(invalid("ambiguous Godot internal-resource identity/offset"));
        }
        v.internal.push(Internal { path, offset });
    }
    let table_end = r.offset;
    if length < 4 { return Err(invalid("truncated Godot resource trailer")); }
    let content_end = length - 4;
    for item in &v.internal {
        if item.offset < table_end || item.offset >= content_end {
            return Err(invalid("Godot internal resource offset exceeds its data region"));
        }
    }
    if r.source.read_at(content_end, 4)? != b"RSRC" {
        return Err(invalid("Godot resource lacks its trailing signature"));
    }
    if v.kind != "AudioStreamWAV" { v.status = "NON_WAV_RESOURCE_PAYLOAD_OPAQUE"; return Ok(v); }
    if !v.external.is_empty() || v.internal.len() != 1 {
        v.status = "REFERENCED_OR_MULTI_RESOURCE_AUDIO_OPAQUE";
        return Ok(v);
    }
    if real64 != 0 || v.flags & 4 != 0 {
        v.status = "REAL64_RESOURCE_PAYLOAD_NOT_PARSED";
        return Ok(v);
    }
    r.offset = v.internal[0].offset;
    r.end = content_end;
    if r.text()? != "AudioStreamWAV" { return Err(invalid("Godot main resource class differs from its header")); }
    let count = r.u32()?;
    if count > MAX_PROPERTIES { return Err(invalid("Godot property count exceeds audio audit limit")); }
    for _ in 0..count {
        super::cancelled()?;
        let name = r.property_name(&v.strings)?;
        if name.is_empty() || v.properties.contains_key(&name) { return Err(invalid("empty/duplicate Godot property name")); }
        if !matches!(name.as_str(), "data" | "format" | "mix_rate" | "stereo" | "loop_mode"
            | "loop_begin" | "loop_end" | "resource_name" | "resource_local_to_scene" | "script") {
            v.status = "UNKNOWN_AUDIO_PROPERTY_SCHEMA";
            return Ok(v);
        }
        let value = match r.u32()? {
            1 => Value::Empty,
            2 => { let n = r.u32()?; if n > 1 { return Err(invalid("invalid Godot boolean property")); } Value::Bool(n != 0) }
            3 => Value::Number(r.u32()? as i32 as i64),
            40 => Value::Number(r.u64()? as i64),
            5 => Value::Text(r.text()?),
            24 => {
                if r.u32()? != 0 { v.status = "OBJECT_REFERENCE_PROPERTY_NOT_PARSED"; return Ok(v); }
                Value::Empty
            }
            31 => {
                let len = r.u32()? as u64;
                let offset = r.offset;
                r.skip(len)?;
                r.skip((4 - len % 4) % 4)?;
                Value::Bytes(offset, len)
            }
            _ => { v.status = "UNSUPPORTED_PROPERTY_VARIANT"; return Ok(v); }
        };
        v.properties.insert(name, value);
    }
    if r.offset != content_end { return Err(invalid("unparsed bytes after the Godot main resource")); }
    if v.properties.get("resource_name").is_some_and(|value| !matches!(value, Value::Text(_)))
        || v.properties.get("resource_local_to_scene").is_some_and(|value| !matches!(value, Value::Bool(_))) {
        return Err(invalid("Godot base-resource property has the wrong variant type"));
    }
    if v.properties.get("script").is_some_and(|value| !matches!(value, Value::Empty)) {
        v.status = "SCRIPTED_AUDIO_RESOURCE_NOT_PARSED";
        return Ok(v);
    }
    // Godot 4.3 class defaults are explicit here; omitted properties are not
    // confused with a missing/corrupt value of a different serialized type.
    let format = number(&v.properties, "format", 0)?;
    let rate = number(&v.properties, "mix_rate", 44100)?;
    let stereo = boolean(&v.properties, "stereo", false)?;
    let loop_mode = number(&v.properties, "loop_mode", 0)?;
    let loop_begin = number(&v.properties, "loop_begin", 0)?;
    let loop_end = number(&v.properties, "loop_end", 0)?;
    let (data_offset, data_bytes) = match v.properties.get("data") {
        Some(Value::Bytes(offset, bytes)) => (*offset, *bytes), None => (0, 0),
        _ => return Err(invalid("Godot audio data is not a packed byte array")),
    };
    if !(0..=3).contains(&format) || rate <= 0 || rate > i32::MAX as i64
        || !(0..=3).contains(&loop_mode) || loop_begin < 0 || loop_end < 0
        || loop_begin > i32::MAX as i64 || loop_end > i32::MAX as i64 {
        return Err(invalid("invalid Godot audio format/rate/loop metadata"));
    }
    let frames = if format <= 1 {
        let frame_bytes = (if format == 0 { 1 } else { 2 }) * (if stereo { 2 } else { 1 });
        if data_bytes % frame_bytes != 0 { return Err(invalid("Godot PCM data is not aligned to its channel/frame size")); }
        let frames = data_bytes / frame_bytes;
        if loop_mode != 0 && (loop_begin as u64 >= loop_end as u64 || loop_end as u64 > frames) {
            return Err(invalid("Godot PCM loop extent exceeds its frame count"));
        }
        Some(frames)
    } else { None }; // ADPCM/QOA framing and seeking are not guessed.
    v.audio = Some(Audio { format, rate, stereo, loop_mode, loop_begin, loop_end,
        data_offset, data_bytes, frames });
    Ok(v)
}

pub fn audit(path: &Path) -> io::Result<()> {
    let mut source = Source::open(path, 512 * 1024 * 1024, MAX_READ)?;
    let v = inspect(&mut source)?;
    source.unchanged()?;
    println!("GODOT_RESOURCE_HEADER|{}|{}|{}|{}|{}|{}", v.major, v.minor, v.format, v.flags, v.uid, field(v.kind.as_bytes()));
    println!("GODOT_RESOURCE_TABLES|{}|{}|{}", v.strings.len(), v.external.len(), v.internal.len());
    for item in &v.external {
        println!("GODOT_RESOURCE_EXTERNAL|{}|{}|{}", field(item.kind.as_bytes()), field(item.path.as_bytes()),
            item.uid.map(|uid| uid.to_string()).unwrap_or_default());
    }
    for item in &v.internal { println!("GODOT_RESOURCE_INTERNAL|{}|{}", field(item.path.as_bytes()), item.offset); }
    for (name, value) in &v.properties {
        let (kind, detail) = match value {
            Value::Number(n) => ("INTEGER", n.to_string()),
            Value::Bool(b) => ("BOOL", u8::from(*b).to_string()),
            Value::Text(text) => ("STRING", field(text.as_bytes())),
            Value::Bytes(_, len) => ("DECLARED_BYTES", len.to_string()),
            Value::Empty => ("EMPTY", String::new()),
        };
        println!("GODOT_AUDIO_PROPERTY|{}|{kind}|{detail}", field(name.as_bytes()));
    }
    if let Some(audio) = &v.audio {
        let codec = match audio.format { 0 => "PCM8", 1 => "PCM16", 2 => "IMA_ADPCM", 3 => "QOA", _ => "UNKNOWN" };
        println!("GODOT_AUDIO_WAV|{codec}|{}|{}|{}|{}|{}|{}|{}|{}", audio.rate,
            if audio.stereo { 2 } else { 1 }, audio.loop_mode, audio.loop_begin, audio.loop_end,
            audio.data_offset, audio.data_bytes, audio.frames.map(|n| n.to_string()).unwrap_or_default());
    }
    println!("GODOT_AUDIO_STATUS|{}|{}", if v.status == "METADATA_ONLY" { "METADATA_ONLY" } else { "OPAQUE" }, v.status);
    eprintln!("Development read-only standalone Godot resource/audio metadata audit; payload bytes are skipped, compressed resources stay opaque, references are not loaded, and no PCK audio writer or waveform/playback validation is implied.");
    Ok(())
}
