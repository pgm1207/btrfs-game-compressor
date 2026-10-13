//! Addressables 1.18.x content-catalogue support for the in-place Unity bundle
//! apply. The apply route may only touch a bundle that belongs to a catalogue
//! after proving, from the shipped bytes, exactly the values the catalogue
//! stores for that bundle; this module locates the catalogue, reads its binary
//! tables, derives its bundle dependency graph, and rewrites only the target
//! entry.
//!
//! Proven scheme (calibrated against a real Unity 2020.3.48f1 /
//! Addressables 1.18.19 / Scriptable Build Pipeline 1.19.2 catalogue, where
//! every one of its bundles reproduced both stored values exactly):
//!   * `m_Crc` is the CRC32 (IEEE, zlib polynomial) of the bundle's
//!     concatenated LZ4-decoded payload, i.e. of the uncompressed content.
//!   * `m_Hash` is `MD5(ContentHash || dependency names)` where `ContentHash`
//!     is `MD5` over the concatenation of each resource file's own 16-byte
//!     MD5; a SerializedFile contributes MD5 bytes from its header data offset
//!     to the end of the node, every other node contributes its whole MD5.
//!     Each dependency name is the dependency bundle's `m_BundleName` plus
//!     `.bundle`, sorted ascending. This is SBP's
//!     `HashingMethods.Calculate(ContentHashes, Dependencies)` with its
//!     default MD5 hasher (the Unity 2020.x/`useV2Hasher=false` default).
//!   * `m_BundleSize` is the bundle file's logical length.
//!   * A sibling `catalog*.hash` file stores the 32-hex `RawHash` of the
//!     catalogue text produced by the same configured hasher; only variants
//!     that reproduce the stored file exactly are accepted.
//!
//! LIMITS (stated, not solved): the scheme is proven per bundle before any
//! write, so a catalogue or bundle shape that does not reproduce its stored
//! values is refused rather than guessed at. The bundle-to-entry match uses
//! the internal id's final component (the bundle file name); duplicate names,
//! catalogue layouts outside the game root, and catalogues without a readable
//! JSON body cannot be proven and are refused. `catalog*.hash` variants other
//! than MD5 over the ASCII/UTF-16 text are refused.
#[cfg(any(test, feature = "development-audits"))]
use std::{
    collections::BTreeSet,
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

#[cfg(any(test, feature = "development-audits"))]
use crate::invalid;

#[cfg(any(test, feature = "development-audits"))]
const MAX_CATALOGUE: u64 = 512 * 1024 * 1024;
#[cfg(any(test, feature = "development-audits"))]
const MAX_TABLE_BYTES: usize = 512 * 1024 * 1024;
#[cfg(any(test, feature = "development-audits"))]
const MAX_ENTRIES: usize = 4 * 1024 * 1024;
#[cfg(any(test, feature = "development-audits"))]
const MAX_WALK_ENTRIES: usize = 1_000_000;
#[cfg(any(test, feature = "development-audits"))]
const MAX_WALK_DEPTH: usize = 64;

/// A catalogue file discovered under the game root.
#[cfg(any(test, feature = "development-audits"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoundCatalogue {
    pub relative: PathBuf,
    pub json: bool,
}

/// One dependency set candidate for a bundle: the sorted SBP-style names of the
/// dependency bundles, e.g. `["865e...bundle", "d902..._unitybuiltinshaders.bundle"]`.
#[cfg(any(test, feature = "development-audits"))]
pub(crate) type DependencyNames = Vec<Vec<u8>>;

/// The catalogue values proven from one bundle's bytes.
#[cfg(any(test, feature = "development-audits"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BundleOptions {
    pub hash: String,
    pub crc: u32,
    pub size: u64,
    pub bundle_name: String,
}

#[cfg(any(test, feature = "development-audits"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HashVariant {
    Md5Ascii,
    Md5Utf16,
}

#[cfg(any(test, feature = "development-audits"))]
#[derive(Debug, Clone)]
pub(crate) struct HashFile {
    pub relative: PathBuf,
    pub variant: HashVariant,
    pub stored: String,
}

#[cfg(any(test, feature = "development-audits"))]
#[derive(Debug, Clone)]
struct EntryData {
    internal_id: usize,
    provider: i32,
    dependency_key: i32,
    dep_hash: i32,
    data_index: i32,
    primary_key: i32,
    resource_type: i32,
}

#[cfg(any(test, feature = "development-audits"))]
#[derive(Debug, Clone)]
struct Bucket {
    entries: Vec<usize>,
}

/// A parsed Addressables JSON content catalogue. The raw file bytes are kept so
/// the caller can prove the file did not change between read and write and so
/// the rewrite can be a pure byte-range splice.
#[cfg(any(test, feature = "development-audits"))]
pub(crate) struct Catalogue {
    pub relative: PathBuf,
    bytes: Vec<u8>,
    internal_ids: Vec<String>,
    entries: Vec<EntryData>,
    buckets: Vec<Bucket>,
    extra: Vec<u8>,
    /// Byte range of the base64 string value of `m_ExtraDataString` in `bytes`.
    extra_value: (usize, usize),
    /// Byte range of the base64 string value of `m_EntryDataString` in `bytes`.
    entry_value: (usize, usize),
    pub hash_file: Option<HashFile>,
}

#[cfg(any(test, feature = "development-audits"))]
impl Catalogue {
    pub(crate) fn bytes(&self) -> &[u8] { &self.bytes }

    /// True when some internal id's final component equals the bundle's file
    /// name. Used only for reporting/coverage; the actual entry match requires
    /// exactly one entry (see `target_entry`).
    pub(crate) fn covers(&self, relative: &Path) -> bool {
        let Some(name) = relative.file_name() else { return false };
        self.internal_ids.iter().any(|id| id.ends_with(".bundle")
            && Path::new(&clean_internal_id(id)).file_name().is_some_and(|candidate| candidate == name))
    }

    /// The single catalogue entry whose internal id names this bundle. `None`
    /// when no entry matches or more than one does (ambiguous: refuse).
    pub(crate) fn target_entry(&self, relative: &Path) -> Option<usize> {
        let name = relative.file_name()?;
        let mut found = None;
        for (index, entry) in self.entries.iter().enumerate() {
            let id = &self.internal_ids[entry.internal_id];
            if !id.ends_with(".bundle") { continue; }
            if Path::new(&clean_internal_id(id)).file_name().is_some_and(|candidate| candidate == name) {
                if found.is_some() { return None; }
                found = Some(index);
            }
        }
        found
    }

    /// The proven options for one entry, or an error naming the unprovable
    /// field. Every field this build cannot reproduce refuses the bundle.
    pub(crate) fn options(&self, entry: usize) -> io::Result<BundleOptions> {
        let data_index = self.entries.get(entry)
            .ok_or_else(|| invalid("Addressables catalogue entry index is out of range"))?.data_index;
        if data_index < 0 {
            return Err(invalid("Addressables catalogue entry carries no AssetBundleRequestOptions blob"));
        }
        let json = self.extra_json(data_index as usize)?;
        let text = utf16_text(json)?;
        let hash = quoted(&text, "m_Hash")?;
        if !hash.is_empty() && (hash.len() != 32 || !hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())) {
            return Err(invalid("Addressables catalogue m_Hash is not 32 lowercase hex digits or empty"));
        }
        let crc = number(&text, "m_Crc")?;
        let crc = u32::try_from(crc).map_err(|_| invalid("Addressables catalogue m_Crc is out of range"))?;
        let size = number(&text, "m_BundleSize")?;
        let size = u64::try_from(size).map_err(|_| invalid("Addressables catalogue m_BundleSize is negative"))?;
        let bundle_name = quoted(&text, "m_BundleName")?;
        Ok(BundleOptions { hash, crc, size, bundle_name })
    }

    /// Distinct dependency-set candidates that contain the target bundle: for
    /// every entry whose dependency bucket names the target, the rest of that
    /// bucket; plus the empty set (a dependency-only bundle with no catalogue
    /// address of its own). A candidate is only usable when every dependency
    /// entry carries an `m_BundleName`.
    pub(crate) fn dependency_candidates(&self, target: usize) -> Vec<DependencyNames> {
        let mut candidates: Vec<DependencyNames> = vec![Vec::new()];
        let mut seen: BTreeSet<Vec<Vec<u8>>> = BTreeSet::new();
        seen.insert(Vec::new());
        for entry in &self.entries {
            if entry.dependency_key < 0 { continue; }
            let Some(bucket) = self.buckets.get(entry.dependency_key as usize) else { continue };
            if !bucket.entries.contains(&target) { continue; }
            let mut names = Vec::new();
            let mut usable = true;
            for dependency in &bucket.entries {
                if *dependency == target { continue; }
                match self.options(*dependency) {
                    Ok(options) if !options.bundle_name.is_empty() => {
                        let mut name = options.bundle_name.into_bytes();
                        name.extend_from_slice(b".bundle");
                        names.push(name);
                    }
                    _ => { usable = false; break; }
                }
            }
            if !usable { continue; }
            names.sort();
            names.dedup();
            if seen.insert(names.clone()) { candidates.push(names); }
        }
        candidates
    }

    /// SBP's final content hash for one bundle: MD5 over the 16-byte content
    /// hash followed by every dependency name in the given order.
    pub(crate) fn projected_hash(content: &[u8; 16], dependencies: &[Vec<u8>]) -> [u8; 16] {
        let mut md5 = crate::md5::Md5::new();
        md5.update(content);
        for dependency in dependencies { md5.update(dependency); }
        md5.finalize()
    }

    /// Game-relative paths for every bundle entry the catalogue names, so the
    /// caller can pick an independent calibration bundle. Internal ids are
    /// `{RuntimePath}\<subdir>\<name>.bundle`; the placeholder resolves to the
    /// catalogue's own directory in the standard local layout. Ids without a
    /// placeholder or without a `.bundle` name are skipped.
    pub(crate) fn covered_paths(&self, limit: usize) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for id in &self.internal_ids {
            if paths.len() >= limit { break; }
            if !id.to_ascii_lowercase().ends_with(".bundle") { continue; }
            let Some((_, tail)) = id.split_once('}') else { continue };
            let tail = tail.replace('\\', "/");
            let tail = tail.trim_start_matches('/');
            if tail.is_empty() { continue; }
            let mut path = self.relative.parent().unwrap_or(Path::new("")).to_path_buf();
            path.push(tail);
            if !paths.contains(&path) { paths.push(path); }
        }
        paths
    }

    /// Contents of a SerializedFile node and every other node, in node order,
    /// folded into the SBP `ContentHash`. The caller provides each node's file
    /// MD5 because the digests are computed from the decoded bundle.
    pub(crate) fn content_hash(file_digests: &[[u8; 16]]) -> [u8; 16] {
        let mut md5 = crate::md5::Md5::new();
        for digest in file_digests { md5.update(digest); }
        md5.finalize()
    }

    /// Rewrite the target entry's `m_Hash`, `m_Crc` and `m_BundleSize` and
    /// return the complete new catalogue bytes. Only the two base64 string
    /// values change: `m_ExtraDataString` (and, when the replacement blob
    /// changes length, `m_EntryDataString` offsets). The caller verifies the
    /// result with `verify_rebuilt` before anything is installed.
    pub(crate) fn rebuild(&self, entry: usize, crc: u32, hash: &str, size: u64) -> io::Result<Vec<u8>> {
        if !hash.is_empty() && (hash.len() != 32 || !hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())) {
            return Err(invalid("refusing to write a non-hex Addressables m_Hash"));
        }
        let data_index = self.entries.get(entry)
            .ok_or_else(|| invalid("Addressables catalogue entry index is out of range"))?.data_index;
        if data_index < 0 {
            return Err(invalid("Addressables catalogue entry carries no AssetBundleRequestOptions blob"));
        }
        let start = data_index as usize;
        let (json_start, old_len) = self.extra_json_range(start)?;
        let mut text = utf16_text(self.extra_json(start)?)?;
        set_quoted(&mut text, "m_Hash", hash)?;
        set_number(&mut text, "m_Crc", crc as i64)?;
        set_number(&mut text, "m_BundleSize", size as i64)?;
        let new_text = encode_utf16(&text);
        let delta = new_text.len() as i64 - old_len as i64;
        let mut extra = self.extra.clone();
        extra.splice(json_start..json_start + old_len, new_text);
        // Every later extra blob moved by `delta` bytes; rewrite the affected
        // entry data offsets so the catalogue stays self-consistent.
        let mut entries = self.entries.clone();
        if delta != 0 {
            for data in &mut entries {
                if data.data_index > data_index { data.data_index += delta as i32; }
            }
        }
        let new_entries = build_entry_data(&entries);
        let new_json = splice_string(&self.bytes, self.entry_value, &base64_encode(&new_entries))?;
        let new_json = splice_string(&new_json, self.extra_value, &base64_encode(&extra))?;
        Ok(new_json)
    }

    /// Prove one rebuilt catalogue: it parses, the target entry carries exactly
    /// the new options, every other entry's options are unchanged, and the bytes
    /// outside the two rewritten base64 values are untouched.
    pub(crate) fn verify_rebuilt(&self, entry: usize, new_bytes: &[u8],
        expected: &BundleOptions) -> io::Result<()> {
        let rebuilt = parse_catalogue(self.relative.clone(), new_bytes.to_vec(), self.hash_file.clone())?;
        // `m_EntryDataString` precedes `m_ExtraDataString` in the catalogue's
        // field order; a two-splice rewrite must leave the prefix, the region
        // between the two values and the suffix byte-identical.
        let (entry_start, entry_len) = self.entry_value;
        let (extra_start, extra_len) = self.extra_value;
        if entry_start > extra_start {
            return Err(invalid("rebuilt Addressables catalogue field order is unexpected"));
        }
        let middle_len = extra_start - (entry_start + entry_len);
        let suffix_len = self.bytes.len() - (extra_start + extra_len);
        let new_middle = rebuilt.bytes.get(entry_start + rebuilt.entry_value.1..)
            .and_then(|tail| tail.get(..middle_len))
            .ok_or_else(|| invalid("rebuilt Addressables catalogue lost its middle field region"))?;
        let new_suffix_start = rebuilt.bytes.len().checked_sub(suffix_len)
            .ok_or_else(|| invalid("rebuilt Addressables catalogue lost its suffix"))?;
        if self.bytes[..entry_start] != rebuilt.bytes[..entry_start] {
            return Err(invalid("rebuilt Addressables catalogue changed bytes before its entry table"));
        }
        if self.bytes[entry_start + entry_len..extra_start] != *new_middle {
            return Err(invalid("rebuilt Addressables catalogue changed bytes between its two fields"));
        }
        if self.bytes[extra_start + extra_len..] != rebuilt.bytes[new_suffix_start..] {
            return Err(invalid("rebuilt Addressables catalogue changed bytes after its extra data"));
        }
        let actual = rebuilt.options(entry)?;
        if &actual != expected {
            return Err(invalid("rebuilt Addressables catalogue entry disagrees with the recomputed values"));
        }
        if self.entries.len() != rebuilt.entries.len() {
            return Err(invalid("rebuilt Addressables catalogue changed its entry count"));
        }
        for index in 0..self.entries.len() {
            if index == entry { continue; }
            if self.options(index).ok() != rebuilt.options(index).ok() {
                return Err(invalid("rebuilt Addressables catalogue changed another bundle entry"));
            }
        }
        Ok(())
    }

    /// Reproduce the stored sibling `catalog*.hash` value from this catalogue's
    /// bytes. `Ok(variant)` is the unique proven variant; `Err` means no
    /// variant reproduced the stored value (refuse).
    pub(crate) fn calibrate_hash_file(&self) -> io::Result<HashVariant> {
        let file = self.hash_file.as_ref()
            .ok_or_else(|| invalid("no Addressables catalogue hash file to calibrate"))?;
        let text = std::str::from_utf8(&self.bytes)
            .map_err(|_| invalid("Addressables catalogue is not valid UTF-8"))?;
        let mut matches = Vec::new();
        for variant in [HashVariant::Md5Ascii, HashVariant::Md5Utf16] {
            if hash_catalogue_text(text, variant).is_some_and(|hash| hex128(&hash) == file.stored) {
                matches.push(variant);
            }
        }
        match matches.len() {
            1 => Ok(matches[0]),
            0 => Err(invalid("Addressables catalogue hash file value cannot be reproduced")),
            _ => Err(invalid("Addressables catalogue hash file value matches more than one variant")),
        }
    }

    /// Recompute the sibling hash file for a rebuilt catalogue with the variant
    /// that was proven against the original file.
    pub(crate) fn recompute_hash_file(&self, variant: HashVariant, new_bytes: &[u8]) -> io::Result<Vec<u8>> {
        let file = self.hash_file.as_ref()
            .ok_or_else(|| invalid("no Addressables catalogue hash file to recompute"))?;
        let text = std::str::from_utf8(new_bytes)
            .map_err(|_| invalid("rebuilt Addressables catalogue is not valid UTF-8"))?;
        let hash = hash_catalogue_text(text, variant)
            .ok_or_else(|| invalid("rebuilt Addressables catalogue cannot be hashed with the proven variant"))?;
        Ok(hex128(&hash).into_bytes())
    }

    /// The byte range of the UTF-16LE JSON text in the extra blob at `start`.
    fn extra_json_range(&self, start: usize) -> io::Result<(usize, usize)> {
        let header = self.extra.get(start).ok_or_else(|| invalid("Addressables extra blob offset is out of range"))?;
        if *header != 7 {
            return Err(invalid("Addressables extra blob is not a JsonObject entry"));
        }
        let mut position = start + 1;
        for kind in 0..2 {
            let length = *self.extra.get(position).ok_or_else(|| invalid("truncated Addressables extra blob"))? as usize;
            position += 1;
            let end = position.checked_add(length).ok_or_else(|| invalid("Addressables extra blob length overflow"))?;
            let name = self.extra.get(position..end).ok_or_else(|| invalid("truncated Addressables extra blob name"))?;
            if kind == 1 && !name.ends_with(b"AssetBundleRequestOptions") {
                return Err(invalid("Addressables extra blob is not an AssetBundleRequestOptions payload"));
            }
            position = end;
        }
        let length_bytes: [u8; 4] = self.extra.get(position..position + 4)
            .ok_or_else(|| invalid("truncated Addressables extra blob length"))?.try_into().unwrap();
        let length = i32::from_le_bytes(length_bytes);
        if length < 0 || length as usize % 2 != 0 {
            return Err(invalid("invalid Addressables extra blob JSON length"));
        }
        position += 4;
        let end = position.checked_add(length as usize).ok_or_else(|| invalid("Addressables extra blob overflow"))?;
        self.extra.get(position..end).ok_or_else(|| invalid("truncated Addressables extra blob JSON"))?;
        Ok((position, length as usize))
    }

    /// The UTF-16LE JSON bytes of the extra blob at `start`.
    fn extra_json(&self, start: usize) -> io::Result<&[u8]> {
        let (offset, length) = self.extra_json_range(start)?;
        Ok(&self.extra[offset..offset + length])
    }
}

/// Load and parse the JSON catalogue at `relative` under `root`. Rejects
/// symlinks, non-regular files, files on another device and oversized bodies.
#[cfg(any(test, feature = "development-audits"))]
pub(crate) fn load(root: &Path, relative: &Path, hash_file: Option<HashFile>) -> io::Result<Catalogue> {
    let path = root.join(relative);
    let root_device = fs::symlink_metadata(root)?.dev();
    let mut current = root.to_path_buf();
    for component in relative.parent().unwrap_or(Path::new("")).components() {
        current.push(component);
        let meta = fs::symlink_metadata(&current)?;
        if !meta.file_type().is_dir() || meta.dev() != root_device {
            return Err(invalid("Addressables catalogue path crosses a symlink or filesystem"));
        }
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.file_type().is_file() || meta.dev() != root_device || meta.len() > MAX_CATALOGUE {
        return Err(invalid("Addressables catalogue is not a regular in-tree file within its size limit"));
    }
    let bytes = fs::read(&path)?;
    parse_catalogue(relative.to_path_buf(), bytes, hash_file)
}

#[cfg(any(test, feature = "development-audits"))]
pub(crate) fn parse_catalogue(relative: PathBuf, bytes: Vec<u8>, hash_file: Option<HashFile>) -> io::Result<Catalogue> {
    if bytes.len() as u64 > MAX_CATALOGUE {
        return Err(invalid("Addressables catalogue exceeds its size limit"));
    }
    let json = std::str::from_utf8(&bytes)
        .map_err(|_| invalid("Addressables catalogue is not valid UTF-8"))?;
    let internal_ids = json_string_array(json, "m_InternalIds")?;
    if internal_ids.len() > MAX_ENTRIES {
        return Err(invalid("Addressables catalogue has too many internal ids"));
    }
    let (extra, extra_value) = json_string_value(json, "m_ExtraDataString")?;
    let (entry_base64, entry_value) = json_string_value(json, "m_EntryDataString")?;
    if extra.len() > MAX_TABLE_BYTES || entry_base64.len() > MAX_TABLE_BYTES {
        return Err(invalid("Addressables catalogue table exceeds its size limit"));
    }
    let extra = base64_decode(extra)?;
    let entry_data = base64_decode(entry_base64)?;
    let buckets = parse_buckets(json)?;
    let entries = parse_entries(&entry_data, &internal_ids)?;
    Ok(Catalogue { relative, bytes, internal_ids, entries, buckets, extra, extra_value, entry_value, hash_file })
}

/// All catalogue files under `root`, in deterministic order. Symlinks are not
/// followed, the restore tree is skipped, and the walk fails closed on its
/// entry/depth budgets rather than guessing whether more files exist.
#[cfg(any(test, feature = "development-audits"))]
pub(crate) fn discover(root: &Path) -> io::Result<Vec<FoundCatalogue>> {
    let root_device = fs::symlink_metadata(root)?.dev();
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), PathBuf::new(), 0usize)];
    let mut seen = 0usize;
    while let Some((directory, relative, depth)) = stack.pop() {
        if depth > MAX_WALK_DEPTH { return Err(invalid("Addressables catalogue search exceeds its depth limit")); }
        for entry in fs::read_dir(&directory)? {
            crate::cancelled()?;
            let entry = entry?;
            seen += 1;
            if seen > MAX_WALK_ENTRIES { return Err(invalid("Addressables catalogue search exceeds its entry limit")); }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.dev() != root_device { continue; }
            if meta.file_type().is_symlink() { continue; }
            if meta.file_type().is_dir() {
                if name == crate::assets::BACKUP_DIR { continue; }
                stack.push((entry.path(), relative.join(&*name), depth + 1));
                continue;
            }
            if !meta.file_type().is_file() { continue; }
            let lower = name.to_ascii_lowercase();
            let json = lower == "catalog.json" || (lower.starts_with("catalog") && lower.ends_with(".json"));
            let hash = lower == "catalog.hash" || (lower.starts_with("catalog") && lower.ends_with(".hash"));
            if json || hash {
                found.push(FoundCatalogue { relative: relative.join(&*name), json });
            }
        }
    }
    found.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(found)
}

/// The sibling hash file for a catalogue, when one exists: same directory, same
/// name stem, `.hash` extension, case-insensitively.
#[cfg(any(test, feature = "development-audits"))]
pub(crate) fn find_hash_file(root: &Path, found: &[FoundCatalogue], catalogue: &Path) -> io::Result<Option<HashFile>> {
    let parent = catalogue.parent().unwrap_or(Path::new(""));
    let Some(stem) = catalogue.file_name().and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once('.').map(|(stem, _)| stem.to_string())) else { return Ok(None) };
    for candidate in found.iter().filter(|candidate| !candidate.json) {
        if candidate.relative.parent().unwrap_or(Path::new("")) != parent { continue; }
        let Some(name) = candidate.relative.file_name().and_then(|name| name.to_str()) else { continue };
        let Some((candidate_stem, _)) = name.rsplit_once('.') else { continue };
        if !candidate_stem.eq_ignore_ascii_case(&stem) { continue; }
        let meta = fs::symlink_metadata(root.join(&candidate.relative))?;
        if !meta.file_type().is_file() || meta.dev() != fs::symlink_metadata(root)?.dev()
            || meta.len() > 1024 {
            return Err(invalid("Addressables catalogue hash file is not a regular in-tree file"));
        }
        let bytes = fs::read(root.join(&candidate.relative))?;
        let stored = std::str::from_utf8(&bytes)
            .map_err(|_| invalid("Addressables catalogue hash file is not ASCII text"))?
            .trim().to_ascii_lowercase();
        if stored.len() != 32 || !stored.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid("Addressables catalogue hash file is not a 128-bit hex value"));
        }
        return Ok(Some(HashFile { relative: candidate.relative.clone(), variant: HashVariant::Md5Ascii, stored }));
    }
    Ok(None)
}

/// True when the game-relative bundle path has an `aa` component.
#[cfg(any(test, feature = "development-audits"))]
pub(crate) fn has_aa_component(relative: &Path) -> bool {
    relative.components().any(|component| matches!(component,
        Component::Normal(name) if name.eq_ignore_ascii_case("aa")))
}

/// True when any discovered catalogue lies in a directory that is `relative`'s
/// directory or one of its ancestors.
#[cfg(any(test, feature = "development-audits"))]
pub(crate) fn catalogue_in_ancestor(found: &[FoundCatalogue], relative: &Path) -> bool {
    let parent = relative.parent().unwrap_or(Path::new(""));
    found.iter().any(|candidate| {
        let directory = candidate.relative.parent().unwrap_or(Path::new(""));
        parent.starts_with(directory)
    })
}

// ---------------------------------------------------------------------------
// JSON helpers. The catalogue is a flat JsonUtility object: field order is
// fixed, values are strings/arrays of strings, and only the base64 fields are
// rewritten. A full JSON parser would silently normalise the file; the splice
// path keeps every other byte identical by construction.
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "development-audits"))]
fn json_string_value<'a>(json: &'a str, name: &str) -> io::Result<(&'a str, (usize, usize))> {
    let needle = format!("\"{name}\":\"");
    let start = json.find(&needle).ok_or_else(|| invalid("Addressables catalogue is missing a required field"))?;
    let value_start = start + needle.len();
    let relative = json[value_start..].find('"')
        .ok_or_else(|| invalid("Addressables catalogue has an unterminated string field"))?;
    let value = &json[value_start..value_start + relative];
    // The base64 alphabet cannot contain a quote or a backslash, so the byte
    // range in `bytes` matches the string slice exactly.
    Ok((value, (value_start, relative)))
}

#[cfg(any(test, feature = "development-audits"))]
fn json_string_array(json: &str, name: &str) -> io::Result<Vec<String>> {
    let needle = format!("\"{name}\":[");
    let start = json.find(&needle).ok_or_else(|| invalid("Addressables catalogue is missing a required array"))?;
    let mut position = start + needle.len();
    let mut values = Vec::new();
    loop {
        let tail = json.get(position..).ok_or_else(|| invalid("truncated Addressables catalogue array"))?;
        let rest = tail.trim_start();
        position += tail.len() - rest.len();
        let Some(rest) = rest.strip_prefix('"') else {
            if rest.starts_with(']') { break; }
            return Err(invalid("Addressables catalogue array holds a non-string value"));
        };
        let (value, consumed) = parse_json_string(rest)?;
        values.push(value);
        position += consumed + 2;
        let tail = json.get(position..).ok_or_else(|| invalid("truncated Addressables catalogue array"))?;
        let rest = tail.trim_start();
        position += tail.len() - rest.len();
        if rest.starts_with(',') { position += 1; continue; }
        if rest.starts_with(']') { break; }
        return Err(invalid("Addressables catalogue array is malformed"));
    }
    Ok(values)
}

#[cfg(any(test, feature = "development-audits"))]
fn parse_json_string(rest: &str) -> io::Result<(String, usize)> {
    let bytes = rest.as_bytes();
    let mut out = String::new();
    let mut position = 0usize;
    while position < bytes.len() {
        match bytes[position] {
            b'"' => return Ok((out, position)),
            b'\\' => {
                position += 1;
                let escape = *bytes.get(position).ok_or_else(|| invalid("truncated Addressables catalogue escape"))?;
                match escape {
                    b'"' | b'\\' | b'/' => out.push(escape as char),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let hex = rest.get(position + 1..position + 5)
                            .ok_or_else(|| invalid("truncated Addressables catalogue unicode escape"))?;
                        let code = u16::from_str_radix(hex, 16)
                            .map_err(|_| invalid("invalid Addressables catalogue unicode escape"))?;
                        out.push(char::from_u32(code as u32)
                            .ok_or_else(|| invalid("unsupported Addressables catalogue surrogate pair"))?);
                        position += 4;
                    }
                    _ => return Err(invalid("unsupported Addressables catalogue escape")),
                }
                position += 1;
            }
            byte if byte < 0x80 => { out.push(byte as char); position += 1; }
            _ => {
                let tail = &rest[position..];
                let ch = tail.chars().next().ok_or_else(|| invalid("invalid Addressables catalogue UTF-8"))?;
                out.push(ch);
                position += ch.len_utf8();
            }
        }
    }
    Err(invalid("unterminated Addressables catalogue string"))
}

// ---------------------------------------------------------------------------
// Addressables binary tables.
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "development-audits"))]
fn parse_entries(data: &[u8], internal_ids: &[String]) -> io::Result<Vec<EntryData>> {
    if data.len() < 4 { return Err(invalid("Addressables entry table is truncated")); }
    let count = i32::from_le_bytes(data[0..4].try_into().unwrap());
    if count < 0 || count as usize > MAX_ENTRIES {
        return Err(invalid("Addressables entry count is invalid"));
    }
    let count = count as usize;
    let expected = 4usize.checked_add(count.checked_mul(28)
        .ok_or_else(|| invalid("Addressables entry table size overflow"))?)
        .ok_or_else(|| invalid("Addressables entry table size overflow"))?;
    if data.len() < expected { return Err(invalid("Addressables entry table is truncated")); }
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let at = 4 + index * 28;
        let field = |offset: usize| -> i32 {
            i32::from_le_bytes(data[at + offset..at + offset + 4].try_into().unwrap())
        };
        let internal_id = field(0);
        let provider = field(4);
        let dependency_key = field(8);
        let dep_hash = field(12);
        let data_index = field(16);
        let primary_key = field(20);
        let resource_type = field(24);
        if internal_id < 0 || internal_id as usize >= internal_ids.len() {
            return Err(invalid("Addressables entry names an out-of-range internal id"));
        }
        entries.push(EntryData { internal_id: internal_id as usize, provider, dependency_key,
            dep_hash, data_index, primary_key, resource_type });
    }
    Ok(entries)
}

#[cfg(any(test, feature = "development-audits"))]
fn parse_buckets(json: &str) -> io::Result<Vec<Bucket>> {
    let (base64, _) = json_string_value(json, "m_BucketDataString")?;
    let data = base64_decode(base64)?;
    if data.len() < 4 || data.len() > MAX_TABLE_BYTES { return Err(invalid("Addressables bucket table size is invalid")); }
    let count = i32::from_le_bytes(data[0..4].try_into().unwrap());
    if count < 0 || count as usize > MAX_ENTRIES { return Err(invalid("Addressables bucket count is invalid")); }
    let mut position = 4usize;
    let mut buckets = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if position + 8 > data.len() { return Err(invalid("Addressables bucket table is truncated")); }
        let _key_offset = i32::from_le_bytes(data[position..position + 4].try_into().unwrap());
        let entries = i32::from_le_bytes(data[position + 4..position + 8].try_into().unwrap());
        position += 8;
        if entries < 0 || entries as usize > MAX_ENTRIES { return Err(invalid("Addressables bucket entry count is invalid")); }
        let end = position.checked_add(entries as usize * 4).ok_or_else(|| invalid("Addressables bucket entry overflow"))?;
        if end > data.len() { return Err(invalid("Addressables bucket entries are truncated")); }
        let mut set = Vec::with_capacity(entries as usize);
        for chunk in data[position..end].chunks_exact(4) {
            let entry = i32::from_le_bytes(chunk.try_into().unwrap());
            if entry >= 0 { set.push(entry as usize); }
        }
        position = end;
        buckets.push(Bucket { entries: set });
    }
    Ok(buckets)
}

#[cfg(any(test, feature = "development-audits"))]
fn build_entry_data(entries: &[EntryData]) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + entries.len() * 28);
    data.extend_from_slice(&(entries.len() as i32).to_le_bytes());
    for entry in entries {
        data.extend_from_slice(&(entry.internal_id as i32).to_le_bytes());
        data.extend_from_slice(&entry.provider.to_le_bytes());
        data.extend_from_slice(&entry.dependency_key.to_le_bytes());
        data.extend_from_slice(&entry.dep_hash.to_le_bytes());
        data.extend_from_slice(&entry.data_index.to_le_bytes());
        data.extend_from_slice(&entry.primary_key.to_le_bytes());
        data.extend_from_slice(&entry.resource_type.to_le_bytes());
    }
    data
}

// ---------------------------------------------------------------------------
// UTF-16 blob text and catalogue hash texts.
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "development-audits"))]
fn utf16_text(bytes: &[u8]) -> io::Result<String> {
    if bytes.len() % 2 != 0 { return Err(invalid("Addressables extra blob JSON is not UTF-16")); }
    let units: Vec<u16> = bytes.chunks_exact(2)
        .map(|pair| u16::from_le_bytes(pair.try_into().unwrap())).collect();
    String::from_utf16(&units).map_err(|_| invalid("Addressables extra blob JSON is not valid UTF-16"))
}

#[cfg(any(test, feature = "development-audits"))]
fn encode_utf16(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() { out.extend_from_slice(&unit.to_le_bytes()); }
    out
}

#[cfg(any(test, feature = "development-audits"))]
fn quoted(text: &str, key: &str) -> io::Result<String> {
    let needle = format!("\"{key}\":\"");
    let start = text.find(&needle).ok_or_else(|| invalid("Addressables options blob is missing a field"))? + needle.len();
    let end = text[start..].find('"').ok_or_else(|| invalid("Addressables options blob has an unterminated string"))?;
    Ok(text[start..start + end].to_string())
}

#[cfg(any(test, feature = "development-audits"))]
fn number(text: &str, key: &str) -> io::Result<i64> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle).ok_or_else(|| invalid("Addressables options blob is missing a field"))? + needle.len();
    let end = start + text[start..].bytes().take_while(|byte| byte.is_ascii_digit() || *byte == b'-').count();
    text[start..end].parse::<i64>().map_err(|_| invalid("Addressables options blob field is not a number"))
}

#[cfg(any(test, feature = "development-audits"))]
fn set_quoted(text: &mut String, key: &str, value: &str) -> io::Result<()> {
    let needle = format!("\"{key}\":\"");
    let start = text.find(&needle).ok_or_else(|| invalid("Addressables options blob is missing a field"))? + needle.len();
    let end = start + text[start..].find('"').ok_or_else(|| invalid("Addressables options blob has an unterminated string"))?;
    text.replace_range(start..end, value);
    Ok(())
}

#[cfg(any(test, feature = "development-audits"))]
fn set_number(text: &mut String, key: &str, value: i64) -> io::Result<()> {
    let needle = format!("\"{key}\":");
    let start = text.find(&needle).ok_or_else(|| invalid("Addressables options blob is missing a field"))? + needle.len();
    let end = start + text[start..].bytes().take_while(|byte| byte.is_ascii_digit() || *byte == b'-').count();
    text.replace_range(start..end, &value.to_string());
    Ok(())
}

#[cfg(any(test, feature = "development-audits"))]
fn splice_string(json: &[u8], value: (usize, usize), replacement: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = json.to_vec();
    out.splice(value.0..value.0 + value.1, replacement.iter().copied());
    Ok(out)
}

/// The catalogue text hashed by `HashingMethods.Calculate(string)`: ASCII bytes
/// when every char is ASCII, UTF-16LE (no BOM) otherwise. `None` when the
/// variant cannot represent the text, so it can never be mistaken for a match.
#[cfg(any(test, feature = "development-audits"))]
fn hash_catalogue_text(text: &str, variant: HashVariant) -> Option<[u8; 16]> {
    let mut md5 = crate::md5::Md5::new();
    match variant {
        HashVariant::Md5Ascii => {
            if !text.is_ascii() { return None; }
            md5.update(text.as_bytes());
        }
        HashVariant::Md5Utf16 => md5.update(&encode_utf16(text)),
    }
    Some(md5.finalize())
}

/// A catalogue internal id uses Unity's Windows-style separators; normalise to
/// the platform separator so the final path component compares cleanly.
#[cfg(any(test, feature = "development-audits"))]
fn clean_internal_id(id: &str) -> String {
    id.replace('\\', "/")
}

/// Lowercase hex for one 128-bit digest, the format every Addressables hash
/// field and file uses.
#[cfg(any(test, feature = "development-audits"))]
pub(crate) fn hex128(bytes: &[u8; 16]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Base64 (standard alphabet, padded), self-contained like the other codecs.
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "development-audits"))]
pub(crate) fn base64_encode(data: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        out.push(ALPHABET[(b[0] >> 2) as usize]);
        out.push(ALPHABET[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize]);
        out.push(if chunk.len() > 1 { ALPHABET[(((b[1] & 0x0f) << 2) | (b[2] >> 6)) as usize] } else { b'=' });
        out.push(if chunk.len() > 2 { ALPHABET[(b[2] & 0x3f) as usize] } else { b'=' });
    }
    out
}

#[cfg(any(test, feature = "development-audits"))]
pub(crate) fn base64_decode(text: &str) -> io::Result<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 4 != 0 { return Err(invalid("Addressables base64 value length is not a multiple of four")); }
    let value = |byte: u8| -> io::Result<u8> {
        Ok(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(invalid("Addressables base64 value has an invalid character")),
        })
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks_exact(4) {
        let last = chunk[3] == b'=';
        let third = chunk[2] == b'=';
        let a = value(chunk[0])?; let b = value(chunk[1])?;
        let c = if third { 0 } else { value(chunk[2])? };
        let d = if last { 0 } else { value(chunk[3])? };
        out.push((a << 2) | (b >> 4));
        if !third { out.push((b << 4) | (c >> 2)); }
        if !last { out.push((c << 6) | d); }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Synthetic Addressables catalogues for tests: real table layouts, no game
    //! files. The stored values are whatever the caller wants, so a fixture can
    //! both calibrate successfully and fail on purpose.
    use super::*;

    pub(crate) struct FixtureEntry {
        pub internal_id: String,
        pub bundle_name: String,
        pub hash: String,
        pub crc: u32,
        pub size: u64,
        pub dependencies: Vec<usize>,
    }

    /// Build a catalogue JSON body whose binary tables follow the Addressables
    /// 1.18.x format. Entries are in the given order; dependency indices point
    /// into that order and are serialized through synthetic dependency-set keys
    /// exactly like `ContentCatalogData.SetData`.
    pub(crate) fn catalogue_json(entries: &[FixtureEntry]) -> Vec<u8> {
        let mut extra = Vec::new();
        let mut extra_indices = Vec::new();
        for entry in entries {
            extra_indices.push(extra.len() as i32);
            let json = format!(
                "{{\"m_Hash\":\"{}\",\"m_Crc\":{},\"m_Timeout\":0,\"m_ChunkedTransfer\":false,\"m_RedirectLimit\":-1,\"m_RetryCount\":0,\"m_BundleName\":\"{}\",\"m_AssetLoadMode\":0,\"m_BundleSize\":{},\"m_UseCrcForCachedBundles\":true,\"m_UWRForLocalBundles\":false}}",
                entry.hash, entry.crc, entry.bundle_name, entry.size);
            extra.push(7u8);
            let assembly = b"Unity.ResourceManager, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null";
            let class = b"UnityEngine.ResourceManagement.ResourceProviders.AssetBundleRequestOptions";
            extra.push(assembly.len() as u8);
            extra.extend_from_slice(assembly);
            extra.push(class.len() as u8);
            extra.extend_from_slice(class);
            let utf16 = encode_utf16(&json);
            extra.extend_from_slice(&(utf16.len() as i32).to_le_bytes());
            extra.extend_from_slice(&utf16);
        }
        // Entries whose dependency list has at least two names share a synthetic
        // dependency-set key; a single dependency uses that dependency's own key.
        let mut keys: Vec<Vec<usize>> = Vec::new();
        let mut per_entry_key: Vec<i32> = vec![-1; entries.len()];
        for (index, entry) in entries.iter().enumerate() {
            if entry.dependencies.is_empty() { continue; }
            let bucket = &entry.dependencies;
            let existing = keys.iter().position(|candidate| candidate == bucket);
            let key = existing.unwrap_or_else(|| { keys.push(bucket.clone()); keys.len() - 1 });
            per_entry_key[index] = key as i32;
        }
        // Primary keys: the internal id's own key. Add one bucket per entry.
        for (index, _entry) in entries.iter().enumerate() {
            if !keys.iter().any(|bucket| *bucket == vec![index]) {
                keys.push(vec![index]);
            }
        }
        let key_of_entry = |index: usize| -> i32 {
            keys.iter().position(|bucket| *bucket == vec![index]).unwrap() as i32
        };
        let mut key_data = Vec::new();
        key_data.extend_from_slice(&(keys.len() as i32).to_le_bytes());
        let mut bucket_data = Vec::new();
        bucket_data.extend_from_slice(&(keys.len() as i32).to_le_bytes());
        for bucket in &keys {
            bucket_data.extend_from_slice(&(key_data.len() as i32).to_le_bytes());
            bucket_data.extend_from_slice(&(bucket.len() as i32).to_le_bytes());
            for entry in bucket { bucket_data.extend_from_slice(&(*entry as i32).to_le_bytes()); }
            // ASCII string objects only.
            let name = if bucket.len() == 1 { format!("key{}", bucket[0]) } else { format!("dep{}", bucket.len()) };
            key_data.push(0);
            key_data.extend_from_slice(&(name.len() as i32).to_le_bytes());
            key_data.extend_from_slice(name.as_bytes());
        }
        let mut entry_data = Vec::new();
        entry_data.extend_from_slice(&(entries.len() as i32).to_le_bytes());
        for (index, _entry) in entries.iter().enumerate() {
            entry_data.extend_from_slice(&(index as i32).to_le_bytes());          // internal id
            entry_data.extend_from_slice(&0i32.to_le_bytes());                    // provider
            entry_data.extend_from_slice(&per_entry_key[index].to_le_bytes());    // dependency key
            entry_data.extend_from_slice(&0i32.to_le_bytes());                    // dep hash
            entry_data.extend_from_slice(&extra_indices[index].to_le_bytes());    // extra data
            entry_data.extend_from_slice(&key_of_entry(index).to_le_bytes());     // primary key
            entry_data.extend_from_slice(&0i32.to_le_bytes());                    // resource type
        }
        let internal_ids: Vec<String> = entries.iter().map(|entry| entry.internal_id.clone()).collect();
        let body = format!(
            "{{\"m_LocatorId\":\"AddressablesMainContentCatalog\",\"m_InternalIds\":{},\"m_KeyDataString\":\"{}\",\"m_BucketDataString\":\"{}\",\"m_EntryDataString\":\"{}\",\"m_ExtraDataString\":\"{}\",\"m_ProviderIds\":[],\"m_resourceTypes\":[],\"m_InternalIdPrefixes\":[]}}",
            json_array(&internal_ids),
            String::from_utf8(base64_encode(&key_data)).unwrap(),
            String::from_utf8(base64_encode(&bucket_data)).unwrap(),
            String::from_utf8(base64_encode(&entry_data)).unwrap(),
            String::from_utf8(base64_encode(&extra)).unwrap());
        body.into_bytes()
    }

    fn json_array(values: &[String]) -> String {
        let mut out = String::from("[");
        for (index, value) in values.iter().enumerate() {
            if index > 0 { out.push(','); }
            out.push('"');
            for ch in value.chars() {
                match ch {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    _ => out.push(ch),
                }
            }
            out.push('"');
        }
        out.push(']');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, hash: &str, crc: u32, size: u64, dependencies: Vec<usize>) -> fixtures::FixtureEntry {
        fixtures::FixtureEntry {
            internal_id: id.to_string(), bundle_name: name.to_string(),
            hash: hash.to_string(), crc, size, dependencies,
        }
    }

    fn catalogue(entries: &[fixtures::FixtureEntry]) -> Catalogue {
        parse_catalogue(PathBuf::from("aa/catalog.json"), fixtures::catalogue_json(entries), None).unwrap()
    }

    #[test]
    fn base64_round_trips_and_rejects_invalid_values() {
        for case in [b"".as_slice(), b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0xff; 257]] {
            let encoded = base64_encode(case);
            let text = std::str::from_utf8(&encoded).unwrap();
            assert_eq!(base64_decode(text).unwrap(), case, "{text}");
        }
        assert!(base64_decode("A===").is_err());
        assert!(base64_decode("AB$C").is_err());
        assert!(base64_decode("AAA").is_err());
    }

    #[test]
    fn parses_entries_buckets_and_dependency_candidates() {
        let entries = [
            entry(r"{RuntimePath}\StandaloneWindows64\aaa.bundle", "aaa", "00000000000000000000000000000000", 1, 10, vec![]),
            entry(r"{RuntimePath}\StandaloneWindows64\bbb.bundle", "bbb", "11111111111111111111111111111111", 2, 20, vec![0]),
            entry("Assets/thing", "unused", "22222222222222222222222222222222", 3, 30, vec![0, 1]),
            entry(r"{RuntimePath}\StandaloneWindows64\ccc.bundle", "ccc", "33333333333333333333333333333333", 4, 40, vec![]),
        ];
        let catalogue = catalogue(&entries);
        assert!(catalogue.covers(Path::new("aa/StandaloneWindows64/aaa.bundle")));
        assert_eq!(catalogue.target_entry(Path::new("aa/StandaloneWindows64/aaa.bundle")), Some(0));
        assert_eq!(catalogue.target_entry(Path::new("aa/StandaloneWindows64/zzz.bundle")), None);
        // Bundle 0 is a dependency of the asset and bundle 1: candidates are the
        // empty set and {bbb}.
        let candidates = catalogue.dependency_candidates(0);
        assert_eq!(candidates.len(), 2, "{candidates:?}");
        assert!(candidates.contains(&Vec::new()));
        assert!(candidates.contains(&vec![b"bbb.bundle".to_vec()]));
        // Bundle 1 depends on nothing else; the asset set gives {aaa} only.
        let candidates = catalogue.dependency_candidates(1);
        assert!(candidates.contains(&vec![b"aaa.bundle".to_vec()]));
        let options = catalogue.options(0).unwrap();
        assert_eq!((options.hash.as_str(), options.crc, options.size, options.bundle_name.as_str()),
            ("00000000000000000000000000000000", 1, 10, "aaa"));
    }

    #[test]
    fn rebuilds_only_the_target_entry_and_verifies() {
        let entries = [
            entry(r"{RuntimePath}\StandaloneWindows64\aaa.bundle", "aaa", "00000000000000000000000000000000", 1, 100, vec![]),
            entry(r"{RuntimePath}\StandaloneWindows64\bbb.bundle", "bbb", "11111111111111111111111111111111", 2, 200, vec![0]),
            entry("Assets/thing", "unused", "22222222222222222222222222222222", 3, 300, vec![0, 1]),
        ];
        let catalogue = catalogue(&entries);
        let before = catalogue.options(1).unwrap();
        let new_hash = "abcdefabcdefabcdefabcdefabcdefab";
        let rebuilt = catalogue.rebuild(1, 4_000_000_000, new_hash, 199).unwrap();
        let expected = BundleOptions {
            hash: new_hash.to_string(), crc: 4_000_000_000, size: 199,
            bundle_name: before.bundle_name.clone(),
        };
        catalogue.verify_rebuilt(1, &rebuilt, &expected).unwrap();
        let parsed = parse_catalogue(PathBuf::from("aa/catalog.json"), rebuilt.clone(), None).unwrap();
        assert_eq!(parsed.options(0).unwrap(), catalogue.options(0).unwrap());
        assert_eq!(parsed.options(2).unwrap(), catalogue.options(2).unwrap());
        assert_eq!(parsed.options(1).unwrap(), expected);
        assert_eq!(parsed.options(1).unwrap().crc.to_string().len(), 10);
        // The bytes outside the two base64 values are untouched even though the
        // entry data offset shifted when the blob grew.
        let old = catalogue.bytes();
        assert_eq!(&old[..old.len().min(64)], &rebuilt[..old.len().min(64)]);
    }

    #[test]
    fn rebuild_refuses_missing_or_unprovable_fields() {
        let entries = [
            entry(r"{RuntimePath}\StandaloneWindows64\aaa.bundle", "aaa", "", 1, 100, vec![]),
            // A blob with no AssetBundleRequestOptions: point a second entry at
            // the same (valid) blob is not possible here, so instead check the
            // option field errors directly through a hand-built catalogue.
        ];
        let catalogue = catalogue(&entries);
        // An empty m_Hash is legal (the build disabled bundle caching).
        assert_eq!(catalogue.options(0).unwrap().hash, "");
        assert!(catalogue.rebuild(0, 2, "not-hex", 99).is_err());
        assert!(catalogue.rebuild(9, 2, "00", 99).is_err());
    }

    #[test]
    fn catalogue_string_array_decodes_escapes() {
        let json = r#"{"m_InternalIds":["a","b\\c","d\"e","\u00e9"]}"#;
        let values = json_string_array(json, "m_InternalIds").unwrap();
        assert_eq!(values, vec!["a", r"b\c", "d\"e", "é"]);
    }
}
