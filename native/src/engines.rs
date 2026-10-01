//! Read-only engine and container detection for the three engines that cover
//! most PC releases (Unity, Unreal, Godot) plus a few common others. It reports
//! where a game's bytes live so the format-aware passes can target the right
//! container; it never parses or rewrites container internals itself.
use std::{
    collections::BTreeMap,
    fs, io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

use super::invalid;

const BACKUP: &str = ".bgc-assets-backup";
const MAX_FILES: u64 = 2_000_000;

#[derive(Default)]
struct Survey {
    godot: bool,
    unityfs: bool,
    files: u64,
    bytes: u64,
    extensions: BTreeMap<String, (u64, u64)>, // ext -> (files, bytes)
    dirs: Vec<String>,                        // lowercased relative dir names
    markers: Vec<String>,                     // lowercased relative file names
    containers: BTreeMap<String, (u64, u64, PathBuf)>, // class -> (files, bytes, example)
}

fn class_of(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    // The extension is already lowercased, so `resS` and `ress` both match.
    Some(match extension {
        "pak" => "pak",
        "ucas" | "utoc" => "unreal-iostore",
        "pck" => "pck",
        "bundle" | "unity3d" | "assetbundle" => "unity-bundle",
        "assets" => "unity-assets",
        "ress" => "unity-stream",
        "resource" => "unity-resource",
        "vtc2" => "amplify-vtc2",
        "bank" | "fsb" => "fmod-audio",
        "wem" | "bnk" => "wwise-audio",
        "awb" => "cri-audio",
        "bik" | "bk2" => "bink-video",
        "usm" | "webm" | "mp4" | "avi" => "video",
        _ => return None,
    })
}

fn survey(dir: &Path, dev: u64, rel: &str, out: &mut Survey, budget: &mut u64) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        if meta.file_type().is_symlink() || meta.dev() != dev {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == BACKUP {
            continue;
        }
        let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        if meta.file_type().is_dir() {
            out.dirs.push(name.to_ascii_lowercase());
            survey(&entry.path(), dev, &child_rel, out, budget)?;
        } else if meta.file_type().is_file() {
            *budget = budget
                .checked_sub(1)
                .ok_or_else(|| invalid("engine scan exceeded its file budget"))?;
            out.files += 1;
            out.bytes += meta.len();
            out.markers.push(name.to_ascii_lowercase());
            if let Some((_, extension)) = name.to_ascii_lowercase().rsplit_once('.') {
                let slot = out.extensions.entry(extension.to_string()).or_default();
                slot.0 += 1;
                slot.1 += meta.len();
            }
            let hash_name = (8..=64).contains(&name.len()) && name.bytes().all(|c| c.is_ascii_hexdigit());
            if let Some(mut class) = class_of(&name).or(if hash_name { Some("unknown-hash-file") } else { None }) {
                // Wwise also uses .pck; the extension alone is not Godot evidence.
                if class == "pck" {
                    let mut magic = [0; 4];
                    let mut source = fs::OpenOptions::new().read(true).custom_flags(0x20000 | 0x800).open(entry.path())?;
                    if source.read_exact(&mut magic).is_ok() {
                        class = match &magic { b"GDPC" => { out.godot = true; "godot-pck" }, b"AKPK" => "wwise-pck", _ => "unknown-pck" };
                    } else { class = "unknown-pck"; }
                } else if class == "unknown-hash-file" || class == "unity-resource" || class == "unity-bundle" {
                    let mut magic = [0; 8];
                    let mut source = fs::OpenOptions::new().read(true).custom_flags(0x20000 | 0x800).open(entry.path())?;
                    if source.read_exact(&mut magic).is_ok() {
                        if &magic == b"UnityFS\0" { out.unityfs = true; class = "unity-bundle"; }
                        else if &magic[..4] == b"FSB5" { class = "fmod-audio"; }
                    }
                    if class == "unknown-hash-file" { continue; }
                }
                let slot = out.containers.entry(class.to_string()).or_default();
                slot.0 += 1;
                slot.1 += meta.len();
                if slot.2.as_os_str().is_empty() {
                    slot.2 = PathBuf::from(&child_rel);
                }
            }
        }
    }
    Ok(())
}

fn detect(out: &Survey) -> Vec<(&'static str, &'static str)> {
    let has_dir = |needle: &str| out.dirs.iter().any(|d| d == needle);
    let has_dir_suffix = |suffix: &str| out.dirs.iter().any(|d| d.ends_with(suffix));
    let has_marker = |needle: &str| out.markers.iter().any(|m| m == needle);
    let has_ext = |ext: &str| out.extensions.contains_key(ext);
    let unity_streams = out
        .extensions
        .get("ress")
        .map(|(f, _)| *f)
        .unwrap_or(0);

    let mut found = Vec::new();
    // Unity: a `*_Data` folder plus the managed runtime or the asset streams.
    if out.unityfs || has_ext("ress") && has_ext("assets")
        || has_marker("unityplayer.dll")
        || has_marker("globalgamemanagers")
        || has_marker("resources.assets")
    {
        let evidence = if has_marker("unityplayer.dll") {
            "UnityPlayer.dll"
        } else if has_marker("globalgamemanagers") {
            "globalgamemanagers"
        } else if unity_streams > 0 {
            ".resS streams"
        } else if out.unityfs {
            "UnityFS bundle signature"
        } else {
            "Assets"
        };
        found.push(("unity", evidence));
    }
    // Unreal: pak archives, and especially the UE5 IoStore pair.
    if has_ext("utoc") || has_ext("ucas") || has_dir_suffix("paks") {
        found.push(("unreal", "IoStore/Paks"));
    } else if has_ext("pak") && (has_dir("engine") || out.markers.iter().any(|m| m.contains("shipping"))) {
        found.push(("unreal", "*.pak + shipping build"));
    }
    // Godot: a packed .pck, or an exported build next to it.
    if out.godot {
        found.push(("godot", "GDPC pack signature"));
    }
    if out.markers.iter().any(|m| m.starts_with("re_chunk_") || m.starts_with("re_engine")) {
        found.push(("re-engine", "re_chunk_*.pak"));
    }
    if has_marker("data.win") {
        found.push(("gamemaker", "data.win"));
    }
    if found.is_empty() {
        let guess = if has_ext("pak") {
            ("unknown", "*.pak")
        } else if unity_streams > 0 {
            ("unity", ".resS streams")
        } else {
            ("unknown", "no known engine marker")
        };
        found.push(guess);
    }
    found
}

/// Print `ENGINE|name|evidence`, then `CONTAINER|class|bytes|files|example`.
pub fn scan(root: &Path) -> io::Result<()> {
    use std::io::Write;
    let meta = fs::symlink_metadata(root)?;
    if !meta.file_type().is_dir() {
        return Err(invalid("engine scan expects a directory"));
    }
    let mut out = Survey::default();
    let mut budget = MAX_FILES;
    survey(root, meta.dev(), "", &mut out, &mut budget)?;
    let mut stdout = io::stdout().lock();
    for (engine, evidence) in detect(&out) {
        writeln!(stdout, "ENGINE|{engine}|{evidence}")?;
    }
    for (class, (files, bytes, example)) in &out.containers {
        let relative = example.strip_prefix(root).unwrap_or(example);
        writeln!(
            stdout,
            "CONTAINER|{class}|{bytes}|{files}|{}",
            relative.display()
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(label: &str) -> PathBuf {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("bgc-engine-{label}-{stamp}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn detected(root: &Path) -> Vec<&'static str> {
        let meta = fs::symlink_metadata(root).unwrap();
        let mut out = Survey::default();
        let mut budget = MAX_FILES;
        survey(root, meta.dev(), "", &mut out, &mut budget).unwrap();
        detect(&out).into_iter().map(|(name, _)| name).collect()
    }

    #[test]
    fn detects_unity_unreal_and_godot_layouts() {
        let unity = fixture("unity");
        fs::create_dir_all(unity.join("Game_Data/Managed")).unwrap();
        fs::write(unity.join("Game_Data/globalgamemanagers"), b"x").unwrap();
        fs::write(unity.join("Game_Data/sharedassets0.assets.resS"), vec![0u8; 10]).unwrap();
        assert!(detected(&unity).contains(&"unity"));
        fs::remove_dir_all(unity).unwrap();

        let unreal = fixture("unreal");
        fs::create_dir_all(unreal.join("Game/Content/Paks")).unwrap();
        fs::create_dir_all(unreal.join("Game/Binaries/Win64")).unwrap();
        fs::write(unreal.join("Game/Content/Paks/game.pak"), vec![0u8; 10]).unwrap();
        fs::write(unreal.join("Game/Binaries/Win64/Game-Win64-Shipping.exe"), vec![0u8; 10]).unwrap();
        assert!(detected(&unreal).contains(&"unreal"));
        fs::remove_dir_all(unreal).unwrap();

        let godot = fixture("godot");
        fs::write(godot.join("data.pck"), b"GDPCfixture").unwrap();
        fs::write(godot.join("game.exe"), vec![0u8; 10]).unwrap();
        assert!(detected(&godot).contains(&"godot"));
        fs::remove_dir_all(godot).unwrap();
    }

    #[test]
    fn classifies_containers_case_insensitively() {
        assert_eq!(class_of("sharedassets12.assets.resS"), Some("unity-stream"));
        assert_eq!(class_of("resources.resource"), Some("unity-resource"));
        assert_eq!(class_of("pakchunk0-WindowsNoEditor.pak"), Some("pak"));
        assert_eq!(class_of("bundle.bundle"), Some("unity-bundle"));
        assert_eq!(class_of("data.unity3d"), Some("unity-bundle"));
        assert_eq!(class_of("textures.vtc2"), Some("amplify-vtc2"));
        assert_eq!(class_of("Game.bnk"), Some("wwise-audio"));
        assert_eq!(class_of("Game.fsb"), Some("fmod-audio"));
        assert_eq!(class_of("Game.awb"), Some("cri-audio"));
        assert_eq!(class_of("voice.wem"), Some("wwise-audio"));
        assert_eq!(class_of("notes.txt"), None);
    }
    #[test]
    fn wwise_pcks_are_not_mistaken_for_godot() {
        let root = fixture("wwise"); fs::write(root.join("voices.pck"), b"AKPKfixture").unwrap();
        assert!(!detected(&root).contains(&"godot")); fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn extensionless_bundles_and_fsb_resource_streams_are_classified() {
        let root = fixture("hash-bundle"); fs::write(root.join("f9285044"), b"UnityFS\0fixture").unwrap();
        fs::write(root.join("audio.resource"), b"FSB5fixture").unwrap();
        let meta = fs::metadata(&root).unwrap(); let mut out = Survey::default(); let mut budget = MAX_FILES;
        survey(&root, meta.dev(), "", &mut out, &mut budget).unwrap();
        assert!(detect(&out).iter().any(|(name, _)| *name == "unity"));
        assert!(out.containers.contains_key("unity-bundle")); assert!(out.containers.contains_key("fmod-audio"));
        fs::remove_dir_all(root).unwrap();
    }
}
