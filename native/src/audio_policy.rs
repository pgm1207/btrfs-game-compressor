//! Audio targets follow the explicit asset-quality profile, not Btrfs Zstd level.
use std::io;

#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub pcm_rate: u32,
    pub pcm_bits: u16,
    pub vorbis_quality: f32,
    pub min_snr_db: f64,
}

pub fn target_for(profile: &str) -> io::Result<Option<Target>> {
    let (pcm_rate, pcm_bits, vorbis_quality, min_snr_db) = match profile {
        "ultra-performance" => (11025, 8, 0.10, 20.0),
        "performance" => (32000, 16, 0.22, 20.0),
        "balanced" => (44100, 16, 0.35, 20.0),
        "quality" => (48000, 16, 0.50, 22.0),
        "ultra-quality" => (48000, 16, 0.65, 24.0),
        "native" | "lossless" => return Ok(None),
        _ => return Err(super::invalid("unknown audio quality profile")),
    };
    Ok(Some(Target { pcm_rate, pcm_bits, vorbis_quality, min_snr_db }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_raise_quality_and_native_lossless_never_transcode() {
        let mut last_rate = 0;
        let mut last_quality = 0.0;
        for p in ["ultra-performance", "performance", "balanced", "quality", "ultra-quality"] {
            let t = target_for(p).unwrap().unwrap();
            assert!(t.pcm_rate >= last_rate);
            assert!(t.vorbis_quality > last_quality);
            assert!(t.min_snr_db >= 20.0);
            last_rate = t.pcm_rate;
            last_quality = t.vorbis_quality;
        }
        assert!(target_for("native").unwrap().is_none());
        assert!(target_for("lossless").unwrap().is_none());
        assert!(target_for("typo").is_err());
    }
}
