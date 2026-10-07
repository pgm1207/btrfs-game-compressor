//! Development read-only FORM chunk framing inventory. Not a general GameMaker
//! parser: runtime version, pointer-object schemas, pixel/audio bytes and code
//! stay opaque. A FORM or data.win name alone does not certify an engine.
use std::{collections::BTreeSet, io, path::Path};
use super::{audit_io::{field, le32, Source}, invalid};

const MAX_CHUNKS: usize = 256;
const MAX_READ: u64 = 1024 * 1024;
const MAX_TABLE: u32 = 4096;

struct Chunk { tag: [u8; 4], start: u64, length: u64 }
struct Table {
    tag: [u8; 4], count: u32, checked: u32, nulls: u32,
    aliases: u32, outside_chunk: u32, table_targets: u32, status: &'static str,
}

fn chunks(source: &mut Source) -> io::Result<Vec<Chunk>> {
    let header = source.read_at(0, 8)?;
    if &header[..4] != b"FORM" { return Err(invalid("not a FORM container")); }
    let length = le32(&header, 4)? as u64;
    if length.checked_add(8) != Some(source.len()) {
        return Err(invalid("FORM length does not exactly match the source"));
    }
    let mut offset = 8u64;
    let mut chunks = Vec::new();
    let mut tags = BTreeSet::new();
    while offset < source.len() {
        super::cancelled()?;
        if chunks.len() >= MAX_CHUNKS { return Err(invalid("FORM chunk count exceeds audit limit")); }
        let bytes = source.read_at(offset, 8)?;
        let tag: [u8; 4] = bytes[..4].try_into().unwrap();
        // No terminal/control bytes in identifiers; unknown printable tags remain
        // opaque and are escaped on output. Never guess their payload schema.
        if tag.iter().any(|b| !(0x20..=0x7e).contains(b)) || !tags.insert(tag) {
            return Err(invalid("invalid or duplicate FORM chunk identifier"));
        }
        let length = le32(&bytes, 4)? as u64;
        let start = offset.checked_add(8).ok_or_else(|| invalid("FORM chunk offset overflow"))?;
        source.range(start, length)?;
        chunks.push(Chunk { tag, start, length });
        // Padding is contained in chunk length for this subset; no heuristic
        // alignment scan or searching forward for plausible chunk tags.
        offset = start.checked_add(length).ok_or_else(|| invalid("FORM chunk length overflow"))?;
    }
    if chunks.is_empty() { return Err(invalid("empty FORM chunk directory")); }
    Ok(chunks)
}

fn table(source: &mut Source, chunk: &Chunk) -> io::Result<Table> {
    if chunk.length < 4 { return Err(invalid("truncated FORM candidate table count")); }
    let count_bytes = source.read_at(chunk.start, 4)?;
    let count = le32(&count_bytes, 0)?;
    let mut result = Table { tag: chunk.tag, count, checked: 0, nulls: 0,
        aliases: 0, outside_chunk: 0, table_targets: 0, status: "POINTER_BOUNDS_ONLY" };
    if count > MAX_TABLE {
        result.status = "TABLE_BUDGET_SKIPPED";
        return Ok(result);
    }
    let bytes = (count as u64).checked_mul(4).and_then(|n| n.checked_add(4))
        .ok_or_else(|| invalid("FORM candidate table length overflow"))?;
    if bytes > chunk.length { return Err(invalid("FORM candidate pointer table exceeds its chunk")); }
    let pointers = source.read_at(chunk.start + 4, (bytes - 4) as usize)?;
    let mut addresses = BTreeSet::new();
    for p in pointers.chunks_exact(4) {
        let address = le32(p, 0)? as u64;
        // Null slots exist in some runtime versions. Without a version schema,
        // report rather than treating a null as an empty texture or a failure.
        if address == 0 {
            result.nulls += 1; result.status = "NULL_SLOTS_OR_VERSIONED_LAYOUT"; continue;
        }
        if address < 8 || address >= source.len() {
            return Err(invalid("FORM candidate object pointer is outside the file"));
        }
        result.checked += 1;
        if !addresses.insert(address) { result.aliases += 1; }
        if address < chunk.start || address >= chunk.start + chunk.length {
            result.outside_chunk += 1;
        } else if address < chunk.start + bytes {
            result.table_targets += 1;
        }
    }
    Ok(result)
}

pub fn audit(path: &Path) -> io::Result<()> {
    let mut source = Source::open(path, u32::MAX as u64 + 8, MAX_READ)?;
    let chunks = chunks(&mut source)?;
    let candidate = chunks.iter().any(|c| &c.tag == b"GEN8")
        && chunks.iter().any(|c| &c.tag == b"STRG");
    let mut tables = Vec::new();
    // Only inspect candidate table framing after independent GameMaker markers.
    // Even then no runtime version is inferred from the file name or chunk list.
    if candidate {
        for chunk in &chunks {
            if matches!(&chunk.tag, b"TXTR" | b"TPAG") { tables.push(table(&mut source, chunk)?); }
        }
    }
    source.unchanged()?;
    println!("FORM_HEADER|{}|{}|{}", source.len(), chunks.len(),
        if candidate { "GAMEMAKER_LAYOUT_CANDIDATE" } else { "UNKNOWN_FORM_LAYOUT" });
    for chunk in &chunks {
        println!("FORM_CHUNK|{}|{}|{}", field(&chunk.tag), chunk.start, chunk.length);
    }
    for table in &tables {
        println!("GAMEMAKER_TABLE|{}|{}|{}|{}", field(&table.tag), table.count, table.checked, table.status);
        println!("GAMEMAKER_POINTER_RISK|{}|{}|{}|{}|{}|OBJECT_SCHEMAS_UNPARSED", field(&table.tag),
            table.nulls, table.aliases, table.outside_chunk, table.table_targets);
    }
    println!("FORM_STATUS|METADATA_ONLY|RUNTIME_AND_PAYLOAD_SCHEMAS_UNPARSED");
    eprintln!("Development read-only FORM framing audit; TXTR/TPAG counts and candidate pointers are not decoded textures, atlas geometry or engine certification. No writer exists.");
    Ok(())
}
