//! Bounded type-tree-guided Texture2D inspection. Never guesses stripped schemas,
//! decodes textures, follows stream paths or writes files. Unsupported trees skip.
use std::{collections::BTreeMap, io};
fn bad(s: &str) -> io::Error { super::invalid(s) }
fn unsupported(s: &str) -> io::Error { io::Error::new(io::ErrorKind::Unsupported, s) }

#[derive(Debug, Clone)]
pub struct Node { kind: String, name: String, size: i32, align: bool, level: u8, end: usize }
fn u32_at(b: &[u8], offset: usize, big: bool) -> u32 {
    let v = b[offset..offset + 4].try_into().unwrap();
    if big { u32::from_be_bytes(v) } else { u32::from_le_bytes(v) }
}
fn local_string_at(strings: &[u8], offset: u32) -> io::Result<&str> {
    let tail = strings.get(offset as usize..).ok_or_else(|| bad("Unity tree string offset out of bounds"))?;
    let n = match tail.iter().take(513).position(|b| *b == 0) {
        Some(n) => n,
        None if tail.len() > 512 => return Err(crate::audit_io::budget_error("Unity tree string length exceeds inspection budget")),
        None => return Err(bad("unterminated Unity tree string")),
    };
    let text = std::str::from_utf8(&tail[..n]).map_err(|_| bad("invalid Unity tree UTF-8"))?;
    if text.is_empty() || text.chars().any(|c| c.is_control() || matches!(c, '|' | '/')) {
        return Err(bad("unsafe Unity tree string"));
    }
    Ok(text)
}
fn string_at(strings: &[u8], offset: u32) -> io::Result<String> {
    // Only common strings needed by this bounded inspector. Unknown common
    // offsets are unsupported, never interpreted as an arbitrary type.
    let common = match offset & 0x7fffffff {
        49 => "Array", 55 => "Base", 76 => "bool", 81 => "char", 106 => "data",
        117 => "double", 161 => "float", 222 => "int", 231 => "long long",
        427 => "m_Name", 789 => "short", 795 => "size", 800 => "SInt16",
        807 => "SInt32", 814 => "SInt64", 821 => "SInt8", 840 => "string",
        874 => "Texture2D", 894 => "TypelessData", 907 => "UInt16", 914 => "UInt32",
        921 => "UInt64", 928 => "UInt8", 934 => "unsigned int", 947 => "unsigned long long",
        966 => "unsigned short", 981 => "vector", 1152 => "FileSize", _ => "",
    };
    if offset & 0x80000000 != 0 {
        if common.is_empty() { return Err(unsupported("unsupported Unity common string")); }
        return Ok(common.to_owned());
    }
    Ok(local_string_at(strings, offset)?.to_owned())
}
pub fn parse(raw: &[u8], strings: &[u8], stride: usize, big: bool) -> io::Result<Vec<Node>> {
    super::cancelled()?;
    if !matches!(stride, 24 | 32) || raw.len() % stride != 0 || raw.is_empty() {
        return Err(bad("unsupported Unity type-tree node bounds"));
    }
    if raw.len() / stride > 4096 { return Err(crate::audit_io::budget_error("Unity tree node count exceeds inspection budget")); }
    // Validate framing/hierarchy BEFORE resolving common-string semantics. An
    // unknown common offset in an early node must not mask malformed later nodes.
    let mut previous_level = None;
    let mut depth_exceeded = false;
    for (index, b) in raw.chunks_exact(stride).enumerate() {
        if index % 256 == 0 { super::cancelled()?; }
        let level = b[2];
        if previous_level.is_none() && level != 0
            || previous_level.is_some_and(|previous: u8| level == 0 || level as u16 > previous as u16 + 1) {
            return Err(bad("invalid Unity tree hierarchy"));
        }
        depth_exceeded |= level > 32;
        previous_level = Some(level);
    }
    // Validate all bounded local strings too. Oversized strings remain a budget
    // skip, but a later out-of-bounds/invalid local string is still malformed.
    let mut string_limit = None;
    for (index, b) in raw.chunks_exact(stride).enumerate() {
        if index % 256 == 0 { super::cancelled()?; }
        for position in [4, 8] {
            let offset = u32_at(b, position, big);
            if offset & 0x80000000 == 0 {
                match local_string_at(strings, offset) {
                    Ok(_) => (),
                    Err(error) if crate::audit_io::is_budget_error(&error) => { string_limit = Some(error); }
                    Err(error) => return Err(error),
                }
            }
        }
    }
    if depth_exceeded { return Err(crate::audit_io::budget_error("Unity tree depth exceeds inspection budget")); }
    if let Some(error) = string_limit { return Err(error); }
    let mut nodes: Vec<Node> = Vec::new();
    let mut string_budget = 128 * 1024usize;
    for b in raw.chunks_exact(stride) {
        let level = b[2];
        let kind = string_at(strings, u32_at(b, 4, big))?;
        let name = string_at(strings, u32_at(b, 8, big))?;
        string_budget = string_budget.checked_sub(kind.len() + name.len())
            .ok_or_else(|| crate::audit_io::budget_error("Unity tree text budget exceeded"))?;
        nodes.push(Node { kind, name,
            size: u32_at(b, 12, big) as i32, align: u32_at(b, 20, big) & 0x4000 != 0, level, end: 0 });
    }
    // Linear subtree-end construction instead of repeatedly searching the
    // remaining node table. The validated depth bounds the active stack to 33.
    let mut active: Vec<usize> = Vec::with_capacity(33);
    for i in 0..nodes.len() {
        if i % 256 == 0 { super::cancelled()?; }
        while active.last().is_some_and(|&parent| nodes[parent].level >= nodes[i].level) {
            nodes[active.pop().unwrap()].end = i;
        }
        active.push(i);
    }
    let end = nodes.len();
    for parent in active { nodes[parent].end = end; }
    if nodes[0].kind != "Texture2D" { return Err(bad("tree is not Texture2D")); }
    Ok(nodes)
}

struct Reader<'a> {
    bytes: &'a [u8], pos: usize, big: bool, work: usize,
    values: BTreeMap<String, Value>, spans: Option<BTreeMap<String, FieldSpan>>,
    span_text_budget: usize,
}
/// Original object-relative bytes only. These spans do not authorize edits:
/// numeric signedness/semantics, dependent fields and all references still need
/// a validated versioned writer schema. Trailing alignment is outside each span.
#[derive(Debug)]
pub struct FieldSpan {
    pub start: usize, pub end: usize, pub kind: &'static str,
    pub payload: Option<(usize, usize)>,
}
pub struct TextureFields {
    pub texture: Texture,
    pub spans: BTreeMap<String, FieldSpan>,
    pub image_count: Option<u64>,
    pub dimension: Option<u64>,
}
#[derive(Debug)]
enum Value { Number(u64), Text(String), Bytes(usize) }
impl Reader<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| bad("Unity texture offset overflow"))?;
        let bytes = self.bytes.get(self.pos..end).ok_or_else(|| bad("truncated Unity texture"))?;
        self.pos = end; Ok(bytes)
    }
    fn number(&mut self, n: usize) -> io::Result<u64> {
        let big = self.big; let bytes = self.take(n)?;
        let mut value = 0u64;
        if big { for b in bytes { value = (value << 8) | *b as u64; } }
        else { for (i, b) in bytes.iter().enumerate() { value |= (*b as u64) << (i * 8); } }
        Ok(value)
    }
    fn align(&mut self) -> io::Result<()> { self.take((4 - self.pos % 4) % 4)?; Ok(()) }
    fn save(&mut self, path: &str, value: Value, capture: bool) -> io::Result<()> {
        if path.len() > 1024 { return Err(crate::audit_io::budget_error("Unity field path exceeds limit")); }
        if capture && self.values.insert(path.to_owned(), value).is_some() { return Err(bad("duplicate Unity texture field")); }
        Ok(())
    }
    fn span(&mut self, path: &str, start: usize, kind: &'static str,
        payload: Option<(usize, usize)>, capture: bool) -> io::Result<()> {
        if !capture { return Ok(()); }
        if !matches!(path, "m_Width" | "m_Height" | "m_TextureFormat" | "m_MipCount"
            | "m_CompleteImageSize" | "m_ImageCount" | "m_TextureDimension"
            | "m_StreamData/offset" | "m_StreamData/size" | "m_StreamData/path"
            | "image data" | "image data/Array") { return Ok(()); }
        if let Some(spans) = &mut self.spans {
            self.span_text_budget = self.span_text_budget.checked_sub(path.len())
                .ok_or_else(|| crate::audit_io::budget_error("Unity field-span text budget exceeded"))?;
            if spans.insert(path.to_owned(), FieldSpan { start, end: self.pos, kind, payload }).is_some() {
                return Err(bad("duplicate Unity field span"));
            }
        }
        Ok(())
    }
    fn walk(&mut self, nodes: &[Node], i: usize, path: &str, capture: bool) -> io::Result<()> {
        self.work = self.work.checked_sub(1)
            .ok_or_else(|| crate::audit_io::budget_error("Unity texture work budget exceeded"))?;
        if self.work % 1024 == 0 { super::cancelled()?; }
        let n = &nodes[i];
        let child = i + 1 < n.end;
        let start = self.pos;
        match n.kind.as_str() {
            "string" => {
                let len = self.number(4)? as usize;
                if len > 4096 { return Err(bad("Unity texture text exceeds limit")); }
                let text = std::str::from_utf8(self.take(len)?).map_err(|_| bad("invalid Unity texture text"))?.to_owned();
                if text.chars().any(|c| c.is_control() || c == '|') { return Err(bad("unsafe Unity texture text")); }
                self.span(path, start, "LENGTH_PREFIXED_UTF8", Some((start + 4, len)), capture)?;
                self.align()?; self.save(path, Value::Text(text), capture)?;
            }
            "TypelessData" => {
                let len = self.number(4)? as usize;
                self.take(len)?;
                self.span(path, start, "LENGTH_PREFIXED_BYTES", Some((start + 4, len)), capture)?;
                self.save(path, Value::Bytes(len), capture)?;
            }
            "Array" => {
                if !child { return Err(bad("Unity array lacks schema")); }
                let size = i + 1; let data = nodes[size].end;
                if nodes[size].name != "size" || nodes[size].kind != "int" || nodes[size].size != 4
                    || data >= n.end || nodes[data].name != "data" || nodes[data].end != n.end {
                    return Err(unsupported("unsupported Unity array schema"));
                }
                let count = self.number(4)? as usize;
                // Byte vectors are skipped in one bounded operation, not copied.
                if nodes[data].end == data + 1 && nodes[data].size == 1
                    && matches!(nodes[data].kind.as_str(), "UInt8" | "SInt8") && !nodes[data].align {
                    self.take(count)?;
                    self.span(path, start, "LENGTH_PREFIXED_BYTES", Some((start + 4, count)), capture)?;
                    self.save(path, Value::Bytes(count), capture)?;
                } else {
                    if count > 100000 { return Err(crate::audit_io::budget_error("Unity array count exceeds limit")); }
                    for _ in 0..count { self.walk(nodes, data, path, false)?; }
                }
            }
            _ if child => {
                let mut j = i + 1;
                while j < n.end {
                    if capture {
                        let length = path.len().checked_add(nodes[j].name.len())
                            .and_then(|length| length.checked_add(usize::from(!path.is_empty())))
                            .ok_or_else(|| bad("Unity field path length overflow"))?;
                        if length > 1024 { return Err(crate::audit_io::budget_error("Unity field path exceeds limit")); }
                        let p = if path.is_empty() { nodes[j].name.clone() } else { format!("{path}/{}", nodes[j].name) };
                        self.walk(nodes, j, &p, true)?;
                    } else {
                        // Skipped array elements need schema walking, not a new
                        // path allocation for every nested field/element.
                        self.walk(nodes, j, path, false)?;
                    }
                    j = nodes[j].end;
                }
            }
            _ => {
                let size = match n.kind.as_str() {
                    "bool" | "UInt8" | "SInt8" => 1,
                    "short" | "SInt16" | "UInt16" | "unsigned short" => 2,
                    "int" | "SInt32" | "UInt32" | "unsigned int" | "float" => 4,
                    "long long" | "SInt64" | "UInt64" | "unsigned long long" | "FileSize" | "double" => 8,
                    _ => return Err(unsupported("unsupported Unity texture leaf type")),
                };
                if n.size != size as i32 { return Err(bad("Unity primitive size mismatch")); }
                let value = self.number(size)?;
                if n.kind == "bool" && value > 1 { return Err(bad("invalid Unity texture boolean")); }
                self.span(path, start, "PRIMITIVE_BITS", None, capture)?;
                self.save(path, Value::Number(value), capture)?;
            }
        }
        if n.align { self.align()?; } Ok(())
    }
}

#[derive(Debug)]
pub struct Texture {
    pub width: u32, pub height: u32, pub format: u32, pub mips: u32,
    pub inline_bytes: usize, pub stream_offset: u64, pub stream_size: u64, pub stream_path: String,
}

/// Names only stable Unity TextureFormat enum values; unknown values remain
/// explicitly unknown rather than being inferred from payload shape.
pub fn format_name(format: u32) -> &'static str {
    match format {
        1 => "Alpha8", 2 => "ARGB4444", 3 => "RGB24", 4 => "RGBA32",
        5 => "ARGB32", 6 => "ARGBFloat", 7 => "RGB565", 8 => "BGR24",
        9 => "R16", 10 => "DXT1", 11 => "DXT3", 12 => "DXT5",
        13 => "RGBA4444", 14 => "BGRA32", 15 => "RHalf", 16 => "RGHalf",
        17 => "RGBAHalf", 18 => "RFloat", 19 => "RGFloat", 20 => "RGBAFloat",
        21 => "YUY2", 22 => "RGB9e5Float", 23 => "RGBFloat", 24 => "BC6H",
        25 => "BC7", 26 => "BC4", 27 => "BC5", 28 => "DXT1Crunched",
        29 => "DXT5Crunched", 30 => "PVRTC_RGB2", 31 => "PVRTC_RGBA2",
        32 => "PVRTC_RGB4", 33 => "PVRTC_RGBA4", 34 => "ETC_RGB4",
        35 => "ATC_RGB4", 36 => "ATC_RGBA8", 41 => "EAC_R",
        42 => "EAC_R_SIGNED", 43 => "EAC_RG", 44 => "EAC_RG_SIGNED",
        45 => "ETC2_RGB", 46 => "ETC2_RGBA1", 47 => "ETC2_RGBA8",
        48 => "ASTC_RGB_4x4", 49 => "ASTC_RGB_5x5", 50 => "ASTC_RGB_6x6",
        51 => "ASTC_RGB_8x8", 52 => "ASTC_RGB_10x10", 53 => "ASTC_RGB_12x12",
        54 => "ASTC_RGBA_4x4", 55 => "ASTC_RGBA_5x5", 56 => "ASTC_RGBA_6x6",
        57 => "ASTC_RGBA_8x8", 58 => "ASTC_RGBA_10x10", 59 => "ASTC_RGBA_12x12",
        60 => "ETC_RGB4_3DS", 61 => "ETC_RGBA8_3DS", 62 => "RG16", 63 => "R8",
        64 => "ETC_RGB4Crunched", 65 => "ETC2_RGBA8Crunched", 66 => "ASTC_HDR_4x4",
        67 => "ASTC_HDR_5x5", 68 => "ASTC_HDR_6x6", 69 => "ASTC_HDR_8x8",
        70 => "ASTC_HDR_10x10", 71 => "ASTC_HDR_12x12", 72 => "RG32",
        73 => "RGB48", 74 => "RGBA64", 75 => "R8_SIGNED", 76 => "RG16_SIGNED",
        77 => "RGB24_SIGNED", 78 => "RGBA32_SIGNED", 79 => "R16_SIGNED",
        80 => "RG32_SIGNED", 81 => "RGB48_SIGNED", 82 => "RGBA64_SIGNED",
        _ => "Unknown",
    }
}
pub fn inspect_texture(nodes: &[Node], bytes: &[u8], big: bool) -> io::Result<Texture> {
    Ok(inspect_fields(nodes, bytes, big, false)?.texture)
}
pub fn inspect_texture_fields(nodes: &[Node], bytes: &[u8], big: bool) -> io::Result<TextureFields> {
    inspect_fields(nodes, bytes, big, true)
}
fn inspect_fields(nodes: &[Node], bytes: &[u8], big: bool, spans: bool) -> io::Result<TextureFields> {
    super::cancelled()?;
    if nodes.is_empty() || nodes[0].kind != "Texture2D" { return Err(bad("missing Texture2D tree")); }
    for field in ["m_Width", "m_Height", "m_TextureFormat", "m_MipCount"] {
        let n = nodes.iter().find(|n| n.level == 1 && n.name == field).ok_or_else(|| unsupported("missing Unity Texture2D schema field"))?;
        if !matches!(n.kind.as_str(), "int" | "SInt32") || n.size != 4 { return Err(unsupported("unsupported Unity Texture2D numeric schema")); }
    }
    let mut r = Reader { bytes, pos: 0, big, work: 200000, values: BTreeMap::new(),
        spans: spans.then(BTreeMap::new), span_text_budget: 128 * 1024 };
    r.walk(nodes, 0, "", true)?;
    if r.pos != bytes.len() { return Err(bad("Unity texture schema does not consume complete object")); }
    let number = |path: &str| -> io::Result<u64> {
        match r.values.get(path) { Some(Value::Number(n)) => Ok(*n), _ => Err(bad("missing numeric Unity texture field")) }
    };
    let width = number("m_Width")?; let height = number("m_Height")?;
    let mips = number("m_MipCount")?; let format = number("m_TextureFormat")?;
    if width == 0 || height == 0 || width > 32768 || height > 32768 || mips == 0 || mips > 16 || format > u32::MAX as u64 {
        return Err(bad("invalid Unity texture dimensions/mip/format"));
    }
    if mips > (32 - (width.max(height) as u32).leading_zeros()) as u64 { return Err(bad("Unity texture has too many mip levels")); }
    let inline_bytes = match r.values.get("image data").or_else(|| r.values.get("image data/Array")) {
        Some(Value::Bytes(n)) => *n, _ => return Err(bad("missing Unity inline image data")),
    };
    let stream_offset = number("m_StreamData/offset")?; let stream_size = number("m_StreamData/size")?;
    let stream_path = match r.values.get("m_StreamData/path") {
        Some(Value::Text(s)) => s.clone(), _ => return Err(bad("missing Unity stream path")),
    };
    if stream_offset.checked_add(stream_size).is_none() || inline_bytes != 0 && stream_size != 0
        || stream_size > 0 && stream_path.is_empty() { return Err(bad("inconsistent Unity texture storage")); }
    super::cancelled()?;
    let optional_number = |name: &str| -> Option<u64> {
        if !nodes.iter().any(|n| n.level == 1 && n.name == name
            && matches!(n.kind.as_str(), "int" | "SInt32") && n.size == 4) {
            return None;
        }
        match r.values.get(name) { Some(Value::Number(n)) => Some(*n), _ => None }
    };
    let image_count = optional_number("m_ImageCount");
    let dimension = optional_number("m_TextureDimension");
    Ok(TextureFields { texture: Texture { width: width as u32, height: height as u32,
        format: format as u32, mips: mips as u32, inline_bytes, stream_offset, stream_size, stream_path },
        spans: r.spans.unwrap_or_default(), image_count, dimension })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn fixture(big: bool) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let specs = [
            (0, "Texture2D", "Base", -1, 0),
            (1, "int", "m_Width", 4, 0), (1, "int", "m_Height", 4, 0),
            (1, "int", "m_TextureFormat", 4, 0), (1, "int", "m_MipCount", 4, 0),
            (1, "TypelessData", "image data", -1, 0x4000),
            (1, "StreamingInfo", "m_StreamData", -1, 0),
            (2, "UInt64", "offset", 8, 0), (2, "UInt32", "size", 4, 0),
            (2, "string", "path", -1, 0x4000),
        ];
        let mut strings = Vec::new(); let mut raw = Vec::new();
        for (level, kind, name, size, flags) in specs {
            let k = strings.len() as u32; strings.extend(kind.as_bytes()); strings.push(0);
            let n = strings.len() as u32; strings.extend(name.as_bytes()); strings.push(0);
            let mut node = vec![0; 32]; node[2] = level;
            for (off, value) in [(4, k), (8, n), (12, size as u32), (20, flags)] {
                node[off..off + 4].copy_from_slice(&if big { value.to_be_bytes() } else { value.to_le_bytes() });
            }
            raw.extend(node);
        }
        let mut b = Vec::new();
        for value in [2048u32, 1024, 10, 12, 0] { b.extend(if big { value.to_be_bytes() } else { value.to_le_bytes() }); }
        b.extend(if big { 4096u64.to_be_bytes() } else { 4096u64.to_le_bytes() });
        for value in [1398120u32, b"shared.assets.resSxx".len() as u32] { b.extend(if big { value.to_be_bytes() } else { value.to_le_bytes() }); }
        b.extend(b"shared.assets.resSxx");
        while b.len() % 4 != 0 { b.push(0); }
        (raw, strings, b)
    }
    #[test]
    fn type_tree_fields_are_read_in_both_endians_without_resolving_paths() {
        for big in [false, true] {
            let (raw, strings, bytes) = fixture(big);
            let tree = parse(&raw, &strings, 32, big).unwrap();
            let t = inspect_texture(&tree, &bytes, big).unwrap();
            assert_eq!((t.width, t.height, t.format, t.mips), (2048, 1024, 10, 12));
            assert_eq!((t.inline_bytes, t.stream_offset, t.stream_size), (0, 4096, 1398120));
            assert_eq!(t.stream_path, "shared.assets.resSxx");
            for n in 0..bytes.len() { assert!(inspect_texture(&tree, &bytes[..n], big).is_err(), "{big}:{n}"); }
            let mut extra = bytes.clone(); extra.push(0); assert!(inspect_texture(&tree, &extra, big).is_err());
        }
    }
    #[test]
    fn bad_hierarchy_strings_dimensions_and_storage_are_rejected() {
        let (raw, strings, bytes) = fixture(false);
        let tree = parse(&raw, &strings, 32, false).unwrap();
        let mut broken = raw.clone(); broken[32 + 2] = 3; assert!(parse(&broken, &strings, 32, false).is_err());
        let mut broken = raw.clone(); broken[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&broken, &strings, 32, false).is_err());
        let mut broken = bytes.clone(); broken[..4].copy_from_slice(&0u32.to_le_bytes());
        assert!(inspect_texture(&tree, &broken, false).is_err());
        let mut broken = bytes.clone(); broken[20..28].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(inspect_texture(&tree, &broken, false).is_err());
        let mut duplicate = tree.clone(); duplicate[2].name = "m_Width".to_owned();
        assert!(inspect_texture(&duplicate, &bytes, false).is_err());
    }
    #[test]
    fn unknown_leaf_and_common_offsets_fail_closed() {
        let (raw, strings, bytes) = fixture(false);
        let mut tree = parse(&raw, &strings, 32, false).unwrap();
        tree[1].kind = "future_type".to_owned(); assert!(inspect_texture(&tree, &bytes, false).is_err());
        assert_eq!(string_at(&[], 0x80000000 | 874).unwrap(), "Texture2D");
        assert!(string_at(&[], 0x80000000 | 123456).is_err());
    }
    #[test]
    fn unknown_common_strings_do_not_hide_later_malformed_tree_structure() {
        for big in [false, true] {
            let (raw, strings, _) = fixture(big);
            let mut unknown = raw.clone();
            let offset = 0x80000000u32 | 123456;
            unknown[4..8].copy_from_slice(&if big { offset.to_be_bytes() } else { offset.to_le_bytes() });
            assert_eq!(parse(&unknown, &strings, 32, big).unwrap_err().kind(), io::ErrorKind::Unsupported);
            let mut broken = unknown.clone();
            broken[32 + 2] = 3;
            assert_eq!(parse(&broken, &strings, 32, big).unwrap_err().kind(), io::ErrorKind::InvalidData);
            let mut broken = unknown;
            let invalid_offset = strings.len() as u32 + 1;
            broken[32 + 4..32 + 8].copy_from_slice(&if big { invalid_offset.to_be_bytes() } else { invalid_offset.to_le_bytes() });
            assert_eq!(parse(&broken, &strings, 32, big).unwrap_err().kind(), io::ErrorKind::InvalidData);
        }
    }
    #[test]
    fn oversized_tree_strings_are_budget_skips_not_proof_of_malformed_storage() {
        let (mut raw, mut strings, _) = fixture(false);
        let offset = strings.len() as u32;
        strings.extend([b'x'; 600]); strings.push(0);
        raw[8..12].copy_from_slice(&offset.to_le_bytes());
        assert!(crate::audit_io::is_budget_error(&parse(&raw, &strings, 32, false).unwrap_err()));
        raw[32 + 2] = 3;
        assert_eq!(parse(&raw, &strings, 32, false).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
    #[test]
    fn texture_format_names_are_bounded_and_unknown_values_stay_unknown() {
        assert_eq!(format_name(10), "DXT1");
        assert_eq!(format_name(25), "BC7");
        assert_eq!(format_name(54), "ASTC_RGBA_4x4");
        assert_eq!(format_name(u32::MAX), "Unknown");
    }
    #[test]
    fn byte_array_image_data_is_skipped_without_allocating_an_element_per_pixel() {
        let (raw, strings, bytes) = fixture(false);
        let mut tree = parse(&raw, &strings, 32, false).unwrap();
        tree[5].kind = "vector".to_owned(); tree[5].align = false;
        tree.splice(6..6, [
            Node { kind: "Array".into(), name: "Array".into(), size: -1, align: true, level: 2, end: 0 },
            Node { kind: "int".into(), name: "size".into(), size: 4, align: false, level: 3, end: 0 },
            Node { kind: "UInt8".into(), name: "data".into(), size: 1, align: false, level: 3, end: 0 },
        ]);
        for i in 0..tree.len() { tree[i].end = (i + 1..tree.len()).find(|&j| tree[j].level <= tree[i].level).unwrap_or(tree.len()); }
        assert_eq!(inspect_texture(&tree, &bytes, false).unwrap().inline_bytes, 0);
        let mut inline = bytes[..16].to_vec(); inline.extend(3u32.to_le_bytes()); inline.extend([1, 2, 3, 0]); inline.extend(&bytes[20..]);
        inline[32..36].copy_from_slice(&0u32.to_le_bytes()); // external size after four bytes of inline storage
        assert_eq!(inspect_texture(&tree, &inline, false).unwrap().inline_bytes, 3);
        inline[16..20].copy_from_slice(&u32::MAX.to_le_bytes()); assert!(inspect_texture(&tree, &inline, false).is_err());
    }
}
