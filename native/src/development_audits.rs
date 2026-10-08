//! Explicit command boundary for draft readers and a detached XNB exporter.
//! Default builds refuse these routes without opening inputs. Neither feature
//! state enables installed apply or automatic container dispatch.
use std::{ffi::OsString, io};
#[cfg(feature = "development-audits")]
use std::path::Path;

const COMMANDS: &[&str] = &[
    "xnb-texture-audit", "vtf-audit", "vpk-audit", "gamemaker-audit",
    "unityfs-texture-inventory", "iostore-index-audit", "godot4-audio-audit",
];
const XNB_EXPORT: &str = "xnb-texture-export";

pub fn dispatch(args: &[OsString]) -> Option<io::Result<()>> {
    let command = args.first()?.to_str()?;
    if command == "--development-audits" {
        if args.len() != 1 {
            return Some(Err(super::invalid("--development-audits takes no arguments")));
        }
        println!("Development asset routes (not in published 0.2.1 binaries).");
        println!("Development reader routes: {}", if cfg!(feature = "development-audits") { "compiled in" } else { "disabled" });
        for name in COMMANDS { println!("  {name} FILE"); }
        println!("  {XNB_EXPORT} MAX_EDGE INPUT OUTPUT (detached experimental export)");
        println!("Draft routes require the development-audits Cargo feature; no installed apply or automatic routing.");
        println!("The XNB export decodes and re-encodes BC pixels; audits do not certify runtime compatibility.");
        println!("Detached XNB publication requires O_TMPFILE and /proc/self/fd; use a trusted research output directory.");
        return Some(Ok(()));
    }
    if !COMMANDS.contains(&command) && command != XNB_EXPORT { return None; }
    if args.len() != if command == XNB_EXPORT { 4 } else { 2 } {
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
            XNB_EXPORT => {
                let edge = args[1].to_str().and_then(|s| s.parse::<u32>().ok())
                    .ok_or_else(|| super::invalid("invalid XNB maximum edge"));
                edge.and_then(|edge| super::xnb::texture_export(edge, Path::new(&args[2]), Path::new(&args[3])))
            }
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
        for command in COMMANDS.iter().copied().chain(std::iter::once(XNB_EXPORT)) {
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
    }
}
