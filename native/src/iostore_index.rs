//! Development-only unencrypted/unsigned IoStore v3–8 metadata addressing audit.
//! No .ucas opening, decompression, directory traversal, package schemas, hash
//! verification or writer. Separate from the existing header-only audit.
//! Layout references are pinned in docs/ENGINE_COMPATIBILITY_NEXT.md.
use std::{collections::{BTreeMap, BTreeSet}, io, path::Path};
use super::{audit_io::{field, le32, Source}, invalid, unreal_iostore::Header};

const MAX_READ: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: u32 = 65536;
const MAX_BLOCKS: u32 = 65536;
const MAX_SEEDS: u32 = 131072;
const MAX_REPORTS: usize = 4096;

struct Block { offset: u64, encoded: u64, decoded: u64, method: u8, partition: u64, local: u64 }
struct Chunk { id: [u8; 12], offset: u64, length: u64, first: Option<usize>, last: Option<usize> }
struct Inventory {
    header: Header, chunks: Vec<Chunk>, blocks: Vec<Block>, methods: Vec<String>,
    partitions: BTreeMap<u64, u64>, kinds: BTreeMap<u8, u64>,
    directory_offset: u64, metadata_offset: u64, metadata_size: u64,
    trailing: u64, status: &'static str,
    ranges: BTreeMap<u64, (usize, super::audit_io::RangeSummary)>,
}

fn big40(bytes: &[u8]) -> io::Result<u64> {
    if bytes.len() != 5 { return Err(invalid("invalid IoStore 40-bit offset/length field")); }
    Ok(bytes.iter().fold(0u64, |value, b| value << 8 | *b as u64))
}
fn little(bytes: &[u8]) -> u64 {
    bytes.iter().enumerate().fold(0u64, |value, (i, b)| value | ((*b as u64) << (i * 8)))
}
fn read_table(source: &mut Source, offset: &mut u64, count: u32, stride: u64) -> io::Result<Vec<u8>> {
    let size = (count as u64).checked_mul(stride).ok_or_else(|| invalid("IoStore table length overflow"))?;
    let bytes = source.read_at(*offset, usize::try_from(size).map_err(|_| invalid("IoStore table size exceeds address space"))?)?;
    *offset = offset.checked_add(size).ok_or_else(|| invalid("IoStore table offset overflow"))?;
    Ok(bytes)
}

fn inspect(source: &mut Source) -> io::Result<Inventory> {
    let prefix = source.read_at(0, 144)?;
    let header = super::unreal_iostore::parse(&prefix)?;
    if header.lower_bound().is_none_or(|n| n > source.len()) {
        return Err(invalid("IoStore header implies more bytes than the TOC contains"));
    }
    let mut v = Inventory { header, chunks: Vec::new(), blocks: Vec::new(), methods: Vec::new(),
        partitions: BTreeMap::new(), kinds: BTreeMap::new(), directory_offset: 0,
        metadata_offset: 0, metadata_size: 0, trailing: 0, status: "METADATA_ONLY", ranges: BTreeMap::new() };
    if !(3..=8).contains(&v.header.version) { v.status = "UNSUPPORTED_ADDRESSING_VERSION"; return Ok(v); }
    if v.header.encrypted() || v.header.signed() {
        v.status = "ENCRYPTED_OR_SIGNED_TABLES_SKIPPED";
        return Ok(v);
    }
    if v.header.entries > MAX_ENTRIES || v.header.blocks > MAX_BLOCKS
        || v.header.perfect_seeds > MAX_SEEDS || v.header.without_perfect > MAX_ENTRIES {
        v.status = "TABLE_BUDGET_SKIPPED";
        return Ok(v);
    }
    if v.header.partitions == 0 || v.header.partitions > 256
        || v.header.partitions > 1 && v.header.partition_size == 0 {
        return Err(invalid("unsupported IoStore partition metadata"));
    }
    if v.header.version < 4 && v.header.perfect_seeds != 0
        || v.header.version < 5 && v.header.without_perfect != 0 {
        return Err(invalid("IoStore perfect-hash counts do not match the TOC version"));
    }
    let mut offset = 144u64;
    let ids = read_table(source, &mut offset, v.header.entries, 12)?;
    let offsets = read_table(source, &mut offset, v.header.entries, 10)?;
    let mut unique = BTreeSet::new();
    for (id, extent) in ids.chunks_exact(12).zip(offsets.chunks_exact(10)) {
        super::cancelled()?;
        let id: [u8; 12] = id.try_into().unwrap();
        if !unique.insert(id) { return Err(invalid("ambiguous duplicate IoStore chunk ID")); }
        *v.kinds.entry(id[11]).or_default() += 1;
        v.chunks.push(Chunk { id, offset: big40(&extent[..5])?, length: big40(&extent[5..])?, first: None, last: None });
    }
    // Seed values are retained opaque: negative overflow sentinels are legitimate.
    // Bounds-check table framing, not the perfect-hash algorithm or lookup result.
    if v.header.version >= 4 {
        read_table(source, &mut offset, v.header.perfect_seeds, 4)?;
    }
    if v.header.version >= 5 {
        let overflow = read_table(source, &mut offset, v.header.without_perfect, 4)?;
        let mut indices = BTreeSet::new();
        for raw in overflow.chunks_exact(4) {
            let index = le32(raw, 0)?;
            if index >= v.header.entries || !indices.insert(index) {
                return Err(invalid("invalid/duplicate IoStore imperfect-hash chunk index"));
            }
        }
    }
    let raw_blocks = read_table(source, &mut offset, v.header.blocks, 12)?;
    let partition_size = if v.header.partitions == 1 && v.header.partition_size == 0 {
        u64::MAX
    } else { v.header.partition_size };
    let mut short_prefix = vec![0u32];
    let mut partition_ranges = BTreeMap::<u64, Vec<(u64, u64)>>::new();
    for raw in raw_blocks.chunks_exact(12) {
        super::cancelled()?;
        // Blocks pack little-endian 40-bit physical offsets and 24-bit sizes.
        // Chunk offsets above use big-endian 40-bit fields: do not share maps.
        let physical = little(&raw[..5]);
        let encoded = little(&raw[5..8]);
        let decoded = little(&raw[8..11]);
        let method = raw[11];
        if encoded == 0 || decoded == 0 || decoded > v.header.block_size as u64
            || method as u32 > v.header.method_count || method == 0 && encoded != decoded {
            return Err(invalid("invalid IoStore compressed-block metadata"));
        }
        let partition = physical / partition_size;
        let local = physical % partition_size;
        let end = local.checked_add(encoded).ok_or_else(|| invalid("IoStore physical block overflow"))?;
        if partition >= v.header.partitions as u64 || end > partition_size {
            return Err(invalid("IoStore block crosses or exceeds declared partitions"));
        }
        let required = v.partitions.entry(partition).or_default();
        *required = (*required).max(end);
        partition_ranges.entry(partition).or_default().push((local, end));
        short_prefix.push(short_prefix.last().copied().unwrap() + u32::from(decoded < v.header.block_size as u64));
        v.blocks.push(Block { offset: physical, encoded, decoded, method, partition, local });
    }
    for (partition, mut ranges) in partition_ranges {
        let summary = super::audit_io::summarize_ranges(&mut ranges)?;
        v.ranges.insert(partition, (ranges.len(), summary));
    }
    let names = read_table(source, &mut offset, v.header.method_count, v.header.method_len as u64)?;
    if v.header.method_count != 0 {
        for raw in names.chunks_exact(v.header.method_len as usize) {
            let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
            if end == 0 || raw[end..].iter().any(|b| *b != 0) {
                return Err(invalid("invalid IoStore compression-method name padding"));
            }
            v.methods.push(field(&raw[..end]));
        }
    }
    let logical_capacity = (v.blocks.len() as u64).checked_mul(v.header.block_size as u64)
        .ok_or_else(|| invalid("IoStore logical addressing overflow"))?;
    for chunk in &mut v.chunks {
        super::cancelled()?;
        let end = chunk.offset.checked_add(chunk.length).ok_or_else(|| invalid("IoStore chunk extent overflow"))?;
        if end > logical_capacity { return Err(invalid("IoStore chunk exceeds logical block address space")); }
        if chunk.length == 0 { continue; }
        let first = (chunk.offset / v.header.block_size as u64) as usize;
        let last = ((end - 1) / v.header.block_size as u64) as usize;
        let local_start = chunk.offset % v.header.block_size as u64;
        let local_end = (end - 1) % v.header.block_size as u64 + 1;
        if first >= v.blocks.len() || last >= v.blocks.len()
            || local_start >= v.blocks[first].decoded || local_end > v.blocks[last].decoded
            || first != last && (v.blocks[first].decoded != v.header.block_size as u64
                || short_prefix[last] != short_prefix[first + 1]) {
            return Err(invalid("IoStore chunk references missing decoded block bytes"));
        }
        chunk.first = Some(first); chunk.last = Some(last);
    }
    if v.header.dir_index_size != 0 && !v.header.indexed() {
        return Err(invalid("IoStore directory extent declared without the indexed flag"));
    }
    v.directory_offset = offset;
    source.range(offset, v.header.dir_index_size as u64)?;
    offset = offset.checked_add(v.header.dir_index_size as u64).ok_or_else(|| invalid("IoStore directory offset overflow"))?;
    v.metadata_offset = offset;
    let meta_stride = if v.header.version >= 8 { 24 } else { 33 };
    let meta = read_table(source, &mut offset, v.header.entries, meta_stride)?;
    for raw in meta.chunks_exact(meta_stride as usize) {
        let flags_offset = if v.header.version >= 8 { 20 } else { 32 };
        if raw[flags_offset] & !3 != 0 { return Err(invalid("unknown IoStore chunk metadata flags")); }
    }
    v.metadata_size = offset - v.metadata_offset;
    v.trailing = source.len().checked_sub(offset).ok_or_else(|| invalid("IoStore metadata exceeds TOC bounds"))?;
    // Trailers are reported, not discarded or guessed as on-demand/source hashes.
    // Hash bytes, directory index contents and perfect-hash lookups stay unverified.
    Ok(v)
}

fn hex_id(id: &[u8; 12]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(24);
    for byte in id { let _ = write!(text, "{byte:02X}"); }
    text
}

pub fn audit(path: &Path) -> io::Result<()> {
    let mut source = Source::open(path, 256 * 1024 * 1024, MAX_READ)?;
    let v = inspect(&mut source)?;
    source.unchanged()?;
    println!("IOSTORE_INDEX_HEADER|{}|{}|{}|{}|{}|{}", v.header.version,
        v.header.entries, v.header.blocks, v.header.flags, v.header.partitions, v.header.partition_size);
    for (index, name) in v.methods.iter().enumerate() { println!("IOSTORE_METHOD|{}|{name}", index + 1); }
    for (kind, count) in &v.kinds { println!("IOSTORE_CHUNK_KIND|{kind}|{count}"); }
    for (index, block) in v.blocks.iter().take(MAX_REPORTS).enumerate() {
        println!("IOSTORE_BLOCK|{index}|{}|{}|{}|{}|{}|{}", block.offset, block.encoded,
            block.decoded, block.method, block.partition, block.local);
    }
    for (index, chunk) in v.chunks.iter().take(MAX_REPORTS).enumerate() {
        let first = chunk.first.map(|n| n.to_string()).unwrap_or_default();
        let last = chunk.last.map(|n| n.to_string()).unwrap_or_default();
        println!("IOSTORE_CHUNK|{index}|{}|{}|{}|{first}|{last}", hex_id(&chunk.id), chunk.offset, chunk.length);
    }
    for (partition, length) in &v.partitions {
        println!("IOSTORE_PARTITION_REQUIRED|{partition}|{length}|COMPANION_NOT_OPENED");
    }
    for (partition, (count, summary)) in &v.ranges {
        println!("IOSTORE_PARTITION_RANGES|{partition}|{count}|{}|{}|{}|{}|DECLARED_PHYSICAL_OFFSETS_ONLY",
            summary.bytes, summary.union, summary.aliases, summary.overlap_groups);
    }
    if v.status == "METADATA_ONLY" {
        println!("IOSTORE_INDEX_SECTIONS|{}|{}|{}|{}|{}", v.directory_offset, v.header.dir_index_size,
            v.metadata_offset, v.metadata_size, v.trailing);
        println!("IOSTORE_INDEX_REPORT_LIMIT|{}|{}|{}|{}", v.chunks.len().min(MAX_REPORTS), v.chunks.len(),
            v.blocks.len().min(MAX_REPORTS), v.blocks.len());
    }
    println!("IOSTORE_INDEX_STATUS|{}|{}", if v.status == "METADATA_ONLY" { "METADATA_ONLY" } else { "OPAQUE" }, v.status);
    eprintln!("Development read-only IoStore addressing audit; directory contents, perfect-hash lookups, hashes/signatures, .ucas bytes and cooked packages are unverified. No extraction or writer exists.");
    Ok(())
}
