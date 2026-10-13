//! Explicit command boundary for draft readers and a detached XNB exporter.
//! Default builds refuse these routes without opening inputs. Neither feature
//! state enables installed apply or automatic container dispatch.
use std::{ffi::OsString, io};
#[cfg(feature = "development-audits")]
use std::path::Path;

const COMMANDS: &[&str] = &[
    "xnb-texture-audit", "vtf-audit", "vpk-audit", "gamemaker-audit",
    "unityfs-texture-inventory", "iostore-index-audit", "godot4-audio-audit",
    "unreal-package-audit", "unreal-texture-audit",
];
const XNB_EXPORT: &str = "xnb-texture-export";
const UNITYFS_TEXTURE_EXPORT: &str = "unityfs-texture-export";
const UNREAL_PAK_ENTRY_EXPORT: &str = "unreal-pak-entry-export";
const UNREAL_TEXTURE_EXPORT: &str = "unreal-texture-export";

pub fn dispatch(args: &[OsString]) -> Option<io::Result<()>> {
    let command = args.first()?.to_str()?;
    if command == "--development-audits" {
        if args.len() != 1 {
            return Some(Err(super::invalid("--development-audits takes no arguments")));
        }
        println!("Development asset routes (not in published 0.3.0 binaries).");
        println!("Development reader routes: {}", if cfg!(feature = "development-audits") { "compiled in" } else { "disabled" });
        for name in COMMANDS { println!("  {name} FILE"); }
        println!("  {XNB_EXPORT} MAX_EDGE INPUT OUTPUT (detached experimental export)");
        println!("  {UNITYFS_TEXTURE_EXPORT} NODE_INDEX:PATH_ID INPUT OUTPUT (detached experimental export)");
        println!("  {UNREAL_PAK_ENTRY_EXPORT} ENTRY_PATH INPUT OUTPUT (detached experimental export)");
        println!("  {UNREAL_TEXTURE_EXPORT} EXPORT_NAME INPUT OUTPUT (detached experimental export)");
        println!("Draft routes require the development-audits Cargo feature; no installed apply or automatic routing.");
        println!("The XNB export decodes and re-encodes BC pixels; audits do not certify runtime compatibility.");
        println!("Detached XNB publication requires O_TMPFILE and /proc/self/fd; use a trusted research output directory.");
        return Some(Ok(()));
    }
    if !COMMANDS.contains(&command) && command != XNB_EXPORT && command != UNITYFS_TEXTURE_EXPORT
        && command != UNREAL_PAK_ENTRY_EXPORT && command != UNREAL_TEXTURE_EXPORT { return None; }
    let arity = if command == XNB_EXPORT || command == UNITYFS_TEXTURE_EXPORT
        || command == UNREAL_PAK_ENTRY_EXPORT || command == UNREAL_TEXTURE_EXPORT { 4 } else { 2 };
    if args.len() != arity {
        return Some(Err(super::invalid("invalid development command arguments; use --development-audits for the draft command list")));
    }
    #[cfg(not(feature = "development-audits"))]
    {
        Some(Err(io::Error::new(io::ErrorKind::Unsupported,
            "unvalidated reader routes are disabled in normal builds; the development-audits Cargo feature is required")))
    }
    #[cfg(feature = "development-audits")]
    {
        let path = Path::new(&args[1]);
        Some(match command {
            "xnb-texture-audit" => super::xnb::texture_audit(path),
            "vtf-audit" => super::vtf::audit(path),
            "vpk-audit" => super::vpk::audit(path),
            "gamemaker-audit" => super::gamemaker::audit(path),
            "unityfs-texture-inventory" => super::unityfs::texture_inventory(path),
            "iostore-index-audit" => super::iostore_index::audit(path),
            "godot4-audio-audit" => super::godot4_audio::audit(path),
            "unreal-package-audit" => super::unreal_package::audit(path),
            "unreal-texture-audit" => super::unreal_texture::audit(path),
            XNB_EXPORT => {
                let edge = args[1].to_str().and_then(|s| s.parse::<u32>().ok())
                    .ok_or_else(|| super::invalid("invalid XNB maximum edge"));
                edge.and_then(|edge| super::xnb::texture_export(edge, Path::new(&args[2]), Path::new(&args[3])))
            }
            UNREAL_PAK_ENTRY_EXPORT => (|| -> io::Result<()> {
                let entry = args[1].to_str().ok_or_else(|| super::invalid("invalid Unreal Pak entry path"))?;
                let input = Path::new(&args[2]);
                let output = Path::new(&args[3]);
                match std::fs::symlink_metadata(output) {
                    Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "Unreal entry export destination exists")),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error),
                }
                let bytes = super::containers::stored_entry(input, entry, 64 * 1024 * 1024)?;
                super::detached_export::publish(output, &bytes, || Ok(()))?;
                println!("UNREAL_PAK_ENTRY|{entry}|{}|EXPORTED", bytes.len());
                eprintln!("Experimental detached Unreal Pak entry extraction: stored legacy entries only, source unchanged. No cooked Texture2D parsing, decompression, signer check or runtime validation.");
                Ok(())
            })(),
            UNITYFS_TEXTURE_EXPORT => (|| -> io::Result<()> {
                let key = args[1].to_str().ok_or_else(|| super::invalid("invalid Unity texture key"))?;
                let (node, id) = key.split_once(':')
                    .ok_or_else(|| super::invalid("Unity texture key must be NODE_INDEX:PATH_ID"))?;
                let node = node.parse::<usize>().map_err(|_| super::invalid("invalid Unity bundle node index"))?;
                let id = id.parse::<u64>().map_err(|_| super::invalid("invalid Unity path ID"))?;
                super::unityfs::texture_export(node, id, Path::new(&args[2]), Path::new(&args[3]))
            })(),
            UNREAL_TEXTURE_EXPORT => (|| -> io::Result<()> {
                let name = args[1].to_str().ok_or_else(|| super::invalid("invalid Unreal texture export name"))?;
                super::unreal_texture::texture_export(name, Path::new(&args[2]), Path::new(&args[3]))
            })(),
            _ => Err(super::invalid("unknown development reader command")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argument_checks_and_unknown_commands_do_not_route_to_sources() {
        assert!(dispatch(&[]).is_none());
        assert!(dispatch(&[OsString::from("not-a-development-command")]).is_none());
        for command in COMMANDS.iter().copied().chain([XNB_EXPORT, UNITYFS_TEXTURE_EXPORT,
            UNREAL_PAK_ENTRY_EXPORT, UNREAL_TEXTURE_EXPORT]) {
            let error = dispatch(&[OsString::from(command)]).unwrap().unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
        let error = dispatch(&[OsString::from("--development-audits"), OsString::from("extra")])
            .unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(not(feature = "development-audits"))]
    #[test]
    fn default_build_refuses_all_drafts_before_input_or_export_edge_processing() {
        for &command in COMMANDS {
            let args = [OsString::from(command), OsString::from("/nonexistent-bgc-guard-input")];
            assert_eq!(dispatch(&args).unwrap().unwrap_err().kind(), io::ErrorKind::Unsupported);
        }
        let args = [OsString::from(XNB_EXPORT), OsString::from("not-a-number"),
            OsString::from("/nonexistent-bgc-guard-input"), OsString::from("/nonexistent-bgc-guard-output")];
        assert_eq!(dispatch(&args).unwrap().unwrap_err().kind(), io::ErrorKind::Unsupported);
        let args = [OsString::from(UNITYFS_TEXTURE_EXPORT), OsString::from("0:1"),
            OsString::from("/nonexistent-bgc-guard-input"), OsString::from("/nonexistent-bgc-guard-output")];
        assert_eq!(dispatch(&args).unwrap().unwrap_err().kind(), io::ErrorKind::Unsupported);
        let args = [OsString::from(UNREAL_PAK_ENTRY_EXPORT), OsString::from("a.b"),
            OsString::from("/nonexistent-bgc-guard-input"), OsString::from("/nonexistent-bgc-guard-output")];
        assert_eq!(dispatch(&args).unwrap().unwrap_err().kind(), io::ErrorKind::Unsupported);
        let args = [OsString::from(UNREAL_TEXTURE_EXPORT), OsString::from("example"),
            OsString::from("/nonexistent-bgc-guard-input"), OsString::from("/nonexistent-bgc-guard-output")];
        assert_eq!(dispatch(&args).unwrap().unwrap_err().kind(), io::ErrorKind::Unsupported);
    }
}
