//! Format-agnostic discovery of redundant asset *variants*: sibling suites that
//! hold the same set of file names but different data, such as architecture
//! builds (`x86`/`x64`), renderer or shader suites, resolution tiers
//! (`720p`/`1080p`/`4K`), language packs and platform folders. Read-only.
//!
//! Nothing here understands any game format. It only compares directory shape,
//! which is what makes it apply to arbitrary games; a human (or a later
//! runtime-log check) decides which member is actually loaded.
use std::{
    collections::HashMap,
    fs, io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

use super::invalid;

const BACKUP: &str = ".bgc-assets-backup";
const MAX_FILES: u64 = 1_000_000;
const MAX_GROUPS: usize = 64;

/// Names that overwhelmingly denote a *variant* rather than real content. Used
/// only to rank and label groups, never to select anything automatically.
fn looks_like_variant(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    const HINTS: [&str; 26] = [
        "720p", "1080p", "1440p", "2160p", "4k", "480p", "360p", "lowres",
        "highres", "low", "medium", "high", "ultra", "bc1", "bc3", "bc5", "bc7",
        "etc1", "etc2", "astc", "dx11", "dx12", "vulkan", "opengl", "x86", "x64",
    ];
    const LANGS: [&str; 30] = [
        "en", "fr", "de", "es", "it", "pt", "ru", "pl", "ja", "ko", "zh", "tr",
        "nl", "sv", "fi", "da", "no", "cs", "hu", "el", "ar", "he", "th", "vi",
        "uk", "id", "ro", "bg", "hr", "sk",
    ];
    const PLATFORMS: [&str; 22] = [
        "windows", "win", "win32", "win64", "mac", "macos", "osx", "linux",
        "linux64", "ps4", "ps5", "playstation", "xbox", "xboxone", "xbsx",
        "switch", "nx", "android", "ios", "mobile", "pellegrino", "vita",
    ];
    let base = lower.rsplit(['.', '-', '_']).next().unwrap_or(&lower);
    HINTS.contains(&lower.as_str())
        || HINTS.contains(&base)
        || LANGS.contains(&lower.as_str())
        || LANGS.contains(&base)
        || PLATFORMS.contains(&lower.as_str())
        || PLATFORMS.contains(&base)
}

/// A group is worth reporting only if it looks like a real variant set (at
/// least one member has a variant-flavoured name) and its members differ in
/// size. This suppresses the common false positive where sequential content
/// directories (levels, regions, hero skins) happen to share file names.
fn reportable(group: &[Member]) -> bool {
    group.iter().any(|m| m.variant) && !group.iter().all(|m| m.bytes == group[0].bytes)
}

#[derive(Default)]
struct Node {
    direct: Vec<String>,
    direct_bytes: u64,
    all: Vec<String>,
    bytes: u64,
    files: u64,
}

fn walk(
    dir: &Path,
    dev: u64,
    nodes: &mut HashMap<PathBuf, Node>,
    budget: &mut u64,
) -> io::Result<Vec<String>> {
    let mut direct = Vec::new();
    let mut direct_bytes = 0u64;
    let mut all = Vec::new();
    let mut bytes = 0u64;
    let mut files = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if meta.file_type().is_dir() {
            if name == BACKUP || meta.dev() != dev {
                continue;
            }
            let sub = walk(&entry.path(), dev, nodes, budget)?;
            let child = nodes.get(&entry.path()).expect("child node");
            bytes += child.bytes;
            files += child.files;
            for path in &sub {
                all.push(format!("{name}/{path}"));
            }
        } else if meta.file_type().is_file() {
            *budget = budget
                .checked_sub(1)
                .ok_or_else(|| invalid("variant scan exceeded its file budget"))?;
            direct.push(name.clone());
            all.push(name);
            direct_bytes += meta.len();
            bytes += meta.len();
            files += 1;
        }
    }
    direct.sort();
    all.sort();
    nodes.insert(
        dir.to_path_buf(),
        Node {
            direct,
            direct_bytes,
            all: all.clone(),
            bytes,
            files,
        },
    );
    Ok(all)
}

#[derive(Clone)]
struct Member {
    path: PathBuf,
    names: Vec<String>,
    bytes: u64,
    files: u64,
    variant: bool,
}

/// Group members for every parent directory whose direct files and/or child
/// directories contain identical file-name sets. Groups are ranked by the
/// bytes that dropping all-but-the-largest member would free.
fn groups(root: &Path) -> io::Result<Vec<Vec<Member>>> {
    let meta = fs::symlink_metadata(root)?;
    if !meta.file_type().is_dir() {
        return Err(invalid("variant scan expects a directory"));
    }
    let mut nodes = HashMap::new();
    let mut budget = MAX_FILES;
    walk(root, meta.dev(), &mut nodes, &mut budget)?;
    let mut result: Vec<Vec<Member>> = Vec::new();
    let mut seen: Vec<Vec<PathBuf>> = Vec::new();
    let mut dirs: Vec<&PathBuf> = nodes.keys().collect();
    dirs.sort();
    for dir in dirs {
        let node = &nodes[dir];
        let mut members = Vec::new();
        if !node.direct.is_empty() {
            members.push(Member {
                path: dir.clone(),
                names: node.direct.clone(),
                bytes: node.direct_bytes,
                files: node.direct.len() as u64,
                variant: false,
            });
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let child = entry.path();
            let Some(info) = nodes.get(&child) else { continue };
            if info.files == 0 {
                continue;
            }
            members.push(Member {
                path: child,
                names: info.all.clone(),
                bytes: info.bytes,
                files: info.files,
                variant: looks_like_variant(&entry.file_name().to_string_lossy()),
            });
        }
        if members.len() < 2 {
            continue;
        }
        // Exact file-name equality keeps the report precise; near-misses are
        // left for a human to judge rather than guessed at.
        let mut buckets: HashMap<&Vec<String>, Vec<usize>> = HashMap::new();
        for (index, member) in members.iter().enumerate() {
            buckets.entry(&member.names).or_default().push(index);
        }
        for indexes in buckets.values() {
            if indexes.len() < 2 || members[indexes[0]].files == 0 {
                continue;
            }
            let mut group: Vec<Member> = indexes.iter().map(|i| members[*i].clone()).collect();
            group.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
            let key: Vec<PathBuf> = group.iter().map(|m| m.path.clone()).collect();
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            result.push(group);
        }
    }
    result.sort_by(|a, b| {
        let free = |g: &Vec<Member>| g.iter().map(|m| m.bytes).sum::<u64>() - g[0].bytes;
        free(b).cmp(&free(a))
    });
    Ok(result)
}

fn basename(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

/// Parse a resolution tier such as `720p`, `1080p` or `4k` into pixels.
fn resolution_rank(name: &str) -> Option<u32> {
    let lower = name.to_ascii_lowercase();
    if lower == "4k" {
        return Some(2160);
    }
    let digits = lower.strip_suffix('p')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Platform folders that never match the build this machine runs (a Windows
/// build under Proton), versus the host platform we must keep.
fn is_host_platform(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "windows" | "win" | "win32" | "win64"
    )
}
fn is_other_platform(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "mac" | "macos" | "osx" | "linux" | "linux64" | "ps4" | "ps5" | "playstation" | "xbox"
            | "xboxone" | "switch" | "nx" | "android" | "ios" | "mobile" | "vita"
    )
}

/// ISO codes and English names mapped to a canonical language code. Only used
/// to recognize a language pack's folder/name inside an otherwise identical
/// sibling group, never to guess at arbitrary directories.
const LANGUAGES: &[(&str, &str)] = &[
    ("en", "en"), ("eng", "en"), ("english", "en"),
    ("fr", "fr"), ("fra", "fr"), ("fre", "fr"), ("french", "fr"),
    ("de", "de"), ("deu", "de"), ("ger", "de"), ("german", "de"),
    ("es", "es"), ("spa", "es"), ("spanish", "es"),
    ("it", "it"), ("ita", "it"), ("italian", "it"),
    ("pt", "pt"), ("por", "pt"), ("portuguese", "pt"),
    ("ru", "ru"), ("rus", "ru"), ("russian", "ru"),
    ("pl", "pl"), ("pol", "pl"), ("polish", "pl"),
    ("ja", "ja"), ("jpn", "ja"), ("japanese", "ja"),
    ("ko", "ko"), ("kor", "ko"), ("korean", "ko"),
    ("zh", "zh"), ("chi", "zh"), ("chinese", "zh"),
    ("chinesesimplified", "zh"), ("chinesetraditional", "zh"),
    ("tr", "tr"), ("tur", "tr"), ("turkish", "tr"),
    ("nl", "nl"), ("dut", "nl"), ("nld", "nl"), ("dutch", "nl"),
    ("sv", "sv"), ("swe", "sv"), ("swedish", "sv"),
    ("fi", "fi"), ("fin", "fi"), ("finnish", "fi"),
    ("da", "da"), ("dan", "da"), ("danish", "da"),
    ("no", "no"), ("nor", "no"), ("norwegian", "no"),
    ("cs", "cs"), ("cze", "cs"), ("ces", "cs"), ("czech", "cs"),
    ("hu", "hu"), ("hun", "hu"), ("hungarian", "hu"),
    ("el", "el"), ("gre", "el"), ("ell", "el"), ("greek", "el"),
    ("ar", "ar"), ("ara", "ar"), ("arabic", "ar"),
    ("he", "he"), ("heb", "he"), ("hebrew", "he"),
    ("th", "th"), ("tha", "th"), ("thai", "th"),
    ("vi", "vi"), ("vie", "vi"), ("vietnamese", "vi"),
    ("uk", "uk"), ("ukr", "uk"), ("ukrainian", "uk"),
    ("id", "id"), ("ind", "id"), ("indonesian", "id"),
    ("ro", "ro"), ("ron", "ro"), ("rum", "ro"), ("romanian", "ro"),
    ("bg", "bg"), ("bul", "bg"), ("bulgarian", "bg"),
    ("hr", "hr"), ("hrv", "hr"), ("croatian", "hr"),
    ("sk", "sk"), ("slk", "sk"), ("slo", "sk"), ("slovak", "sk"),
];

/// Return the canonical language code for a folder/name such as `en`, `fr_FR`,
/// `English(US)`, `es-419` or `ChineseTraditional`.
fn language_of(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let core = lower
        .split(|c: char| c == '(' || c == ')' || c == ' ' || c == '.' || c == '-' || c == '_')
        .find(|part| !part.is_empty())?;
    LANGUAGES
        .iter()
        .find(|(token, _)| *token == core)
        .map(|(_, code)| *code)
}

/// Split a file stem such as `voiceover_en` into its language-free base
/// (`voiceover`) and the language code. Requires at least a base and a token so
/// a bare `en`/`no` file name is not mistaken for a language pack.
fn split_language(stem: &str) -> Option<(String, &'static str)> {
    let tokens: Vec<&str> = stem
        .split(|c| c == '_' || c == '-' || c == '.' || c == ' ')
        .filter(|part| !part.is_empty())
        .collect();
    if tokens.len() < 2 {
        return None;
    }
    let mut language = None;
    let mut base = Vec::new();
    for token in &tokens {
        if language.is_none() {
            if let Some(code) = language_of(token) {
                language = Some(code);
                continue;
            }
        }
        base.push(*token);
    }
    let language = language?;
    Some((base.join("_"), language))
}

struct FileMember {
    path: PathBuf,
    lang: &'static str,
    bytes: u64,
}

/// Find sibling files in the same directory that differ only by a language
/// token (`voiceover_en` / `voiceover_es`), the file-level counterpart to a
/// language folder.
fn file_language_groups(root: &Path) -> io::Result<Vec<Vec<FileMember>>> {
    let dev = fs::symlink_metadata(root)?.dev();
    let mut result = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut buckets: HashMap<(String, String), Vec<FileMember>> = HashMap::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() || meta.dev() != dev {
                continue;
            }
            if meta.file_type().is_dir() {
                if entry.file_name() != BACKUP {
                    stack.push(entry.path());
                }
                continue;
            }
            if !meta.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let (stem, extension) = match name.rsplit_once('.') {
                Some((stem, extension)) => (stem.to_string(), extension.to_ascii_lowercase()),
                None => (name, String::new()),
            };
            if let Some((base, lang)) = split_language(&stem) {
                buckets
                    .entry((base, extension))
                    .or_default()
                    .push(FileMember {
                        path: entry.path(),
                        lang,
                        bytes: meta.len(),
                    });
            }
        }
        for members in buckets.into_values() {
            let distinct: std::collections::HashSet<&str> =
                members.iter().map(|m| m.lang).collect();
            if distinct.len() >= 2 {
                result.push(members);
            }
        }
    }
    Ok(result)
}

/// Canonicalize a user-supplied language token (a code or an English name) to
/// the same short code the detector produces, so comparisons always match.
pub fn canonical_language(token: &str) -> String {
    language_of(token)
        .map(str::to_string)
        .unwrap_or_else(|| token.trim().to_ascii_lowercase())
}

/// What kind of fallback a member name denotes. Only these are ever eligible.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fallback {
    Resolution(u32),
    HostPlatform,
    OtherPlatform,
    TextureFormat,
    Language(&'static str),
}

fn fallback_kind(name: &str) -> Option<Fallback> {
    if let Some(rank) = resolution_rank(name) {
        return Some(Fallback::Resolution(rank));
    }
    let lower = name.to_ascii_lowercase();
    if is_host_platform(&lower) {
        return Some(Fallback::HostPlatform);
    }
    if is_other_platform(&lower) {
        return Some(Fallback::OtherPlatform);
    }
    const TEXTURE: [&str; 13] = [
        "bc1", "bc2", "bc3", "bc4", "bc5", "bc6", "bc7", "etc1", "etc2", "astc",
        "dxt1", "dxt5", "pvr",
    ];
    if TEXTURE.contains(&lower.as_str()) {
        return Some(Fallback::TextureFormat);
    }
    if let Some(code) = language_of(name) {
        return Some(Fallback::Language(code));
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
pub struct Removal {
    pub path: PathBuf,
    pub bytes: u64,
}

/// Decide which members of each variant group can be removed offline while the
/// game stays playable, using only names and layout:
///   * resolution ladders: keep the highest tier (or an unqualified base),
///   * platform folders: keep the host platform, drop the others,
///   * language packs: drop any whose code is not in `keep_languages` (an empty
///     list disables language handling entirely).
/// Architecture builds and ambiguous groups are deliberately left alone.
/// Nothing is removed here; this only plans.
pub fn slim_plan(root: &Path, keep_languages: &[String]) -> io::Result<Vec<Removal>> {
    let mut removals = Vec::new();
    // File-level language packs (voiceover_en / voiceover_es) alongside the
    // directory groups handled below.
    if !keep_languages.is_empty() {
        for group in file_language_groups(root)? {
            if !group
                .iter()
                .any(|member| keep_languages.iter().any(|want| want == member.lang))
            {
                continue;
            }
            for member in group {
                if !keep_languages.iter().any(|want| want == member.lang) {
                    removals.push(Removal {
                        path: member.path,
                        bytes: member.bytes,
                    });
                }
            }
        }
    }
    for group in groups(root)? {
        if group.len() < 2 {
            continue;
        }
        // The base member is the one whose path is an ancestor of every other.
        let base = (0..group.len()).find(|i| {
            group
                .iter()
                .enumerate()
                .all(|(j, other)| j == *i || (other.path.starts_with(&group[*i].path) && other.path != group[*i].path))
        });
        let variants: Vec<usize> = (0..group.len()).filter(|i| Some(*i) != base).collect();
        if variants.is_empty() {
            continue;
        }
        // Every variant member must be a recognized fallback kind, otherwise
        // the group is left alone (languages, level folders, arch builds...).
        let kinds: Vec<Fallback> = match variants
            .iter()
            .map(|i| fallback_kind(basename(&group[*i].path)))
            .collect::<Option<Vec<_>>>()
        {
            Some(kinds) => kinds,
            None => continue,
        };
        // A pure language group is handled separately, respecting the keep set.
        if kinds.iter().all(|k| matches!(k, Fallback::Language(_))) {
            if keep_languages.is_empty() {
                continue;
            }
            let kept = |code: &str| keep_languages.iter().any(|want| want == code);
            // Only act when at least one language in this group is kept, so a
            // group can never be emptied (which could break the engine).
            if !kinds
                .iter()
                .any(|k| matches!(k, Fallback::Language(code) if kept(code)))
            {
                continue;
            }
            for (i, kind) in variants.iter().zip(&kinds) {
                if let Fallback::Language(code) = kind {
                    if !kept(code) {
                        removals.push(Removal {
                            path: group[*i].path.clone(),
                            bytes: group[*i].bytes,
                        });
                    }
                }
            }
            continue;
        }
        // A host-platform folder is the one this machine runs, never a fallback.
        if kinds.contains(&Fallback::HostPlatform) && base.is_some() {
            continue;
        }
        // Never bulk-remove a group that also contains language packs; those
        // need the keep set and are safer left for an explicit decision.
        if kinds.iter().any(|k| matches!(k, Fallback::Language(_))) {
            continue;
        }
        if base.is_some() {
            // An unqualified base is the default/full tier, so every suffixed
            // member is a fallback (covers Hades' `Packages` + `720p` + `BC3`).
            for i in &variants {
                removals.push(Removal {
                    path: group[*i].path.clone(),
                    bytes: group[*i].bytes,
                });
            }
            continue;
        }
        let all_resolution = kinds.iter().all(|k| matches!(k, Fallback::Resolution(_)));
        let all_platform = kinds
            .iter()
            .all(|k| matches!(k, Fallback::HostPlatform | Fallback::OtherPlatform));
        if all_resolution {
            let max = kinds
                .iter()
                .filter_map(|k| match k {
                    Fallback::Resolution(rank) => Some(*rank),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            for (i, kind) in variants.iter().zip(&kinds) {
                if !matches!(kind, Fallback::Resolution(rank) if *rank >= max) {
                    removals.push(Removal {
                        path: group[*i].path.clone(),
                        bytes: group[*i].bytes,
                    });
                }
            }
        } else if all_platform
            && kinds.contains(&Fallback::HostPlatform)
        {
            for (i, kind) in variants.iter().zip(&kinds) {
                if *kind != Fallback::HostPlatform {
                    removals.push(Removal {
                        path: group[*i].path.clone(),
                        bytes: group[*i].bytes,
                    });
                }
            }
        }
    }
    removals.sort_by(|a, b| a.path.cmp(&b.path));
    removals.dedup_by(|a, b| a.path == b.path);
    Ok(removals)
}

/// Print one `SLIM|bytes|path` record per planned removal. Read-only.
pub fn slim(root: &Path, keep_languages: &[String]) -> io::Result<()> {
    use std::io::Write;
    let mut out = io::stdout().lock();
    for removal in slim_plan(root, keep_languages)? {
        let relative = removal
            .path
            .strip_prefix(root)
            .map_err(|_| invalid("slim path escaped root"))?;
        writeln!(out, "SLIM|{}|{}", removal.bytes, relative.display())?;
    }
    Ok(())
}

/// Print one `VARIANT` record per redundant group:
/// `VARIANT|bytes|files|flag|path|bytes|files|flag|path|...`
/// where `flag` is `1` when the member's directory name looks like a variant.
pub fn scan(root: &Path) -> io::Result<()> {
    use std::io::Write;
    let mut out = io::stdout().lock();
    for group in groups(root)?.into_iter().take(MAX_GROUPS) {
        if !reportable(&group) {
            continue;
        }
        let mut line = String::from("VARIANT");
        for member in &group {
            let relative = member
                .path
                .strip_prefix(root)
                .map_err(|_| invalid("variant path escaped root"))?;
            let shown = if relative.as_os_str().is_empty() {
                ".".to_string()
            } else {
                relative.display().to_string()
            };
            line.push_str(&format!(
                "|{}|{}|{}|{}",
                member.bytes,
                member.files,
                u8::from(member.variant),
                shown
            ));
        }
        writeln!(out, "{line}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(label: &str) -> PathBuf {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("bgc-variants-{label}-{stamp}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn sizes(root: &Path) -> Vec<usize> {
        let mut found: Vec<usize> = groups(root).unwrap().into_iter().map(|g| g.len()).collect();
        found.sort_unstable();
        found
    }

    #[test]
    fn finds_resolution_and_architecture_suites_and_ignores_unrelated_dirs() {
        let base = fixture("detect");
        for suite in ["", "720p", "BC3"] {
            let dir = base.join("Packages").join(suite);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("A.pkg"), vec![1u8; 100 + suite.len()]).unwrap();
            fs::write(dir.join("B.pkg"), vec![2u8; 200 + suite.len()]).unwrap();
        }
        for arch in ["x86", "x64", "x64Vk"] {
            let dir = base.join(arch);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("game.exe"), vec![3u8; 50 + arch.len()]).unwrap();
        }
        fs::create_dir_all(base.join("Audio")).unwrap();
        fs::write(base.join("Audio/voice.fsb"), b"audio").unwrap();
        fs::create_dir_all(base.join("Maps")).unwrap();
        fs::write(base.join("Maps/level.bin"), b"map").unwrap();
        assert_eq!(sizes(&base), vec![3, 3]);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn reports_language_packs_and_marks_variantish_names() {
        let base = fixture("lang");
        for suite in ["lang_en", "lang_fr", "lang_de"] {
            fs::create_dir_all(base.join(suite)).unwrap();
            fs::write(base.join(suite).join("strings.txt"), format!("same-ish {suite}")).unwrap();
        }
        let found = groups(&base).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].len(), 3);
        assert!(found[0].iter().any(|m| m.variant));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn slim_keeps_highest_resolution_and_host_platform_only() {
        let base = fixture("slim");
        // Resolution ladder without an unqualified base -> keep 1080p, drop 720p.
        for (tier, size) in [("720p", 100u64), ("1080p", 300)] {
            let dir = base.join("Packages").join(tier);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("A.pkg"), vec![1u8; size as usize]).unwrap();
        }
        // Platform folders -> keep Windows, drop Mac/PS4.
        for platform in ["Windows", "Mac", "PS4"] {
            let dir = base.join("Audio").join(platform);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("bank.fsb"), vec![2u8; 50 + platform.len()]).unwrap();
        }
        // Level folders, and language packs when no language is selected, must
        // be left untouched.
        for entry in ["en", "fr", "Level_Cave", "Level_Desert"] {
            let dir = base.join("Misc").join(entry);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("data.bin"), vec![3u8; 40]).unwrap();
        }
        let plan = slim_plan(&base, &[]).unwrap();
        let paths: Vec<String> = plan.iter().map(|r| r.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert!(paths.contains(&"720p".to_string()), "{paths:?}");
        assert!(!paths.contains(&"1080p".to_string()), "{paths:?}");
        assert!(paths.contains(&"Mac".to_string()) && paths.contains(&"PS4".to_string()));
        assert!(!paths.contains(&"Windows".to_string()));
        assert!(!paths.iter().any(|p| p.starts_with("Level_") || p == "en" || p == "fr"), "{paths:?}");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn slim_keeps_unqualified_base_and_drops_all_resolution_fallbacks() {
        let base = fixture("slim-base");
        let plain = base.join("Packages");
        fs::create_dir_all(&plain).unwrap();
        fs::write(plain.join("A.pkg"), vec![9u8; 400]).unwrap();
        for tier in ["720p", "BC3"] {
            let dir = plain.join(tier);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("A.pkg"), vec![9u8; 100]).unwrap();
        }
        let plan = slim_plan(&base, &[]).unwrap();
        let paths: Vec<String> = plan.iter().map(|r| r.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(paths, vec!["720p", "BC3"]);
        assert!(plan.iter().all(|r| !r.path.ends_with("Packages")));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn language_packs_follow_the_selected_languages() {
        let base = fixture("lang-slim");
        for (name, size) in [("English(US)", 100u64), ("French", 200), ("German", 300)] {
            let dir = base.join("Audio/Localized").join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("voice.fsb"), vec![7u8; size as usize]).unwrap();
        }
        // Keeping only English removes the other two.
        let keep = vec!["en".to_string()];
        let plan = slim_plan(&base, &keep).unwrap();
        let mut names: Vec<String> = plan.iter().map(|r| r.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["French", "German"]);
        // Selecting a language absent from this group leaves it fully intact so
        // a group can never be emptied.
        assert!(slim_plan(&base, &["ja".to_string()]).unwrap().is_empty());
        // An empty keep list disables language handling entirely.
        assert!(slim_plan(&base, &[]).unwrap().is_empty());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn file_level_language_packs_follow_the_selected_languages() {
        let base = fixture("file-lang");
        fs::create_dir_all(base.join("banks")).unwrap();
        for (name, size) in [("voiceover_en.bundle", 300u64), ("voiceover_es.bundle", 200)] {
            fs::write(base.join("banks").join(name), vec![1u8; size as usize]).unwrap();
        }
        // A file whose name merely contains a short token must not be grouped.
        fs::write(base.join("banks").join("no.bundle"), b"unrelated").unwrap();
        fs::write(base.join("banks").join("it.bundle"), b"unrelated").unwrap();
        let plan = slim_plan(&base, &["en".to_string()]).unwrap();
        let names: Vec<String> = plan
            .iter()
            .map(|r| r.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["voiceover_es.bundle"]);
        // Removing the only kept language is never allowed; with `es` kept, `en`
        // is dropped; with a language absent here, nothing changes.
        assert!(slim_plan(&base, &["ja".to_string()]).unwrap().is_empty());
        assert!(slim_plan(&base, &[]).unwrap().is_empty());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn language_aliases_and_suffixes_map_to_one_code() {
        for (name, code) in [
            ("en", "en"),
            ("fr_FR", "fr"),
            ("German", "de"),
            ("es-419", "es"),
            ("English(US)", "en"),
            ("zh-Hans", "zh"),
            ("Portuguese", "pt"),
        ] {
            assert_eq!(language_of(name), Some(code), "{name}");
        }
        assert_eq!(language_of("Level_Cave"), None);
        assert_eq!(language_of("Textures"), None);
    }

    #[test]
    fn reportable_rejects_level_like_and_identical_sized_groups() {
        let mk = |name: &str, bytes: u64, variant: bool| Member {
            path: PathBuf::from(name),
            names: vec!["a".into()],
            bytes,
            files: 1,
            variant,
        };
        // Sequential level folders share file names but are not variants.
        assert!(!reportable(&[mk("s01", 100, false), mk("s02", 200, false)]));
        // A parent plus a variant-flavoured child is a real candidate.
        assert!(reportable(&[mk("Packages", 100, false), mk("Packages/720p", 200, true)]));
        // Identical byte totals are duplication, not variants.
        assert!(!reportable(&[mk("a", 100, true), mk("b", 100, true)]));
    }

    #[test]
    fn ignores_identical_size_groups_and_symlinked_dirs() {
        use std::os::unix::fs::symlink;
        let base = fixture("identical");
        for suite in ["a", "b"] {
            fs::create_dir_all(base.join(suite)).unwrap();
            fs::write(base.join(suite).join("same.bin"), vec![9u8; 64]).unwrap();
        }
        // Equal byte totals across members indicate duplication, not variants,
        // so the printed report stays quiet even though the group is detected.
        assert_eq!(sizes(&base), vec![2]);
        symlink(base.join("a"), base.join("a-link")).unwrap();
        assert_eq!(sizes(&base), vec![2]);
        fs::remove_dir_all(base).unwrap();
    }
}
