//! Shared conservative quality guards, independent of the selected lossy tier.
//! Dimensions cannot establish semantic purpose (UI, card, background, etc.).
//! Preserve known atlases rather than treating their sheet size as sprite size.
use std::path::Path;

pub const SMALL_EDGE: u32 = 512;
pub const MIN_SHORT_EDGE: u32 = 64;

/// Profile caps are preferences, not permission to destroy most of a texture's
/// original detail. Use original/logical dimensions, never the already-resized
/// payload, so applying the same tier repeatedly cannot halve it repeatedly.
pub fn effective_edge(profile_edge: u32, original_w: u32, original_h: u32) -> u32 {
    profile_edge.max(original_w.max(original_h).div_ceil(2))
}

/// Avoid aggressive palette reduction on unknown-purpose assets. JPEG/BC codecs
/// still have their own lossy encoding; this guard is for explicit RGB rounding.
pub fn color_bits(requested: u8) -> u8 { requested.max(7) }

pub fn small_or_thin(w: u32, h: u32) -> bool {
    w.max(h) <= SMALL_EDGE || w.min(h) <= MIN_SHORT_EDGE
}

pub fn output_too_thin(w: u32, h: u32) -> bool {
    w.min(h) < MIN_SHORT_EDGE
}

/// A conservative naming hint, not proof of layout. False positives save quality
/// at the expense of space. Metadata references provide additional protection.
pub fn atlas_hint(path: &Path) -> bool {
    path.to_string_lossy().to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| matches!(word, "atlas" | "atlases" | "spritesheet" | "spritesheets"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_and_atlas_hints_are_conservative() {
        assert!(small_or_thin(512, 512));
        assert!(small_or_thin(4096, 64));
        assert!(!small_or_thin(513, 65));
        assert!(output_too_thin(640, 63));
        assert!(!output_too_thin(640, 64));
        assert!(atlas_hint(Path::new(".godot/imported/card_atlas_0.png-hash.ctex")));
        assert!(atlas_hint(Path::new("res://atlases/sheet.png")));
        assert!(!atlas_hint(Path::new("res://background.png")));
    }

    #[test]
    fn reduction_budget_is_relative_and_monotonic_across_profiles() {
        for edge in [640, 1280, 1920, 2560, 3840] {
            assert!(effective_edge(edge, 4096, 2048) >= 2048);
            assert!(effective_edge(edge, 8191, 6000) >= 4096);
        }
        assert_eq!(effective_edge(640, 1000, 600), 640);
        assert_eq!(effective_edge(1920, 4096, 2048), 2048);
        assert_eq!(effective_edge(3840, 4096, 2048), 3840);
        assert_eq!(color_bits(4), 7);
        assert_eq!(color_bits(8), 8);
    }
}
