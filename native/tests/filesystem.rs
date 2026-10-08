//! Opt-in, destructive only to newly created fixtures: BGC_TEST_BTRFS_DIR=/path cargo test.
use std::{
    env, fs,
    os::unix::fs::{symlink, MetadataExt},
    path::Path,
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bgc-native"))
        .args(args)
        .env_remove("BGC_VERBOSE")
        .output()
        .unwrap()
}
fn success(args: &[&str]) -> String {
    let out = run(args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
fn read_only_asset_plan_works_without_a_btrfs_mount() {
    // This test intentionally uses the ordinary temporary directory. It must
    // exercise the real native planner, not a shell/mock Btrfs backend.
    let root = env::temp_dir().join(format!(
        "bgc-any-filesystem-{}",
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let unknown = root.join("untouched.bin");
    let input = b"asset planning must not write to the source";
    fs::write(&unknown, input).unwrap();
    let path = root.to_str().unwrap();
    let output = run(&["asset-plan", "balanced", path]);
    assert!(
        output.status.success(),
        "asset-plan should not require Btrfs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("ASSETS|plan|Balanced (1080p)|0|"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(fs::read(&unknown).unwrap(), input);
    assert!(!root.join(".bgc-assets-backup").exists());

    // Neither dangling symlinks nor an aliased root are allowed to trigger a
    // surprising write or incorrect scan.
    let alias = root.with_extension("symlink");
    symlink(&root, &alias).unwrap();
    assert!(!run(&["asset-plan", "balanced", alias.to_str().unwrap()])
        .status.success());
    fs::remove_file(alias).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn no_backup_assets_replace_atomically_without_recovery_files() {
    let Some(parent) = env::var_os("BGC_TEST_BTRFS_DIR") else { return; };
    let root = std::path::PathBuf::from(parent).join(format!("bgc-no-backup-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir(&root).unwrap();
    let path = root.join("large.png");
    image::RgbImage::from_pixel(3840, 2160, image::Rgb([17, 31, 49])).save(&path).unwrap();
    let original = fs::read(&path).unwrap();
    let output = success(&["assets", "apply-no-backup", "balanced", "1", root.to_str().unwrap()]);
    assert!(output.contains("ASSETS|apply|Balanced (1080p)|1|"), "{output}");
    assert!(!root.join(".bgc-assets-backup").exists());
    assert_ne!(fs::read(&path).unwrap(), original);
    assert_eq!(image::image_dimensions(&path).unwrap(), (1920, 1080));
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn texture_compress_exports_a_smaller_dds_and_leaves_the_source() {
    let parent = env::var_os("BGC_TEST_BTRFS_DIR").map(std::path::PathBuf::from).unwrap_or_else(env::temp_dir);
    let root = parent.join(format!("bgc-texture-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir(&root).unwrap();
    // A 64x64 legacy uncompressed BGRA DDS: 32-bit masks, single mip.
    let mut dds = vec![0u8; 128 + 64 * 64 * 4];
    dds[..4].copy_from_slice(b"DDS ");
    dds[4..8].copy_from_slice(&124u32.to_le_bytes());
    dds[12..16].copy_from_slice(&64u32.to_le_bytes()); // height
    dds[16..20].copy_from_slice(&64u32.to_le_bytes()); // width
    dds[20..24].copy_from_slice(&(64 * 4u32).to_le_bytes()); // pitch
    dds[28..32].copy_from_slice(&1u32.to_le_bytes()); // mips
    dds[76..80].copy_from_slice(&32u32.to_le_bytes());
    dds[80..84].copy_from_slice(&0x41u32.to_le_bytes()); // RGB | ALPHAPIXELS
    dds[88..92].copy_from_slice(&32u32.to_le_bytes());
    dds[92..96].copy_from_slice(&0x00ff0000u32.to_le_bytes());
    dds[96..100].copy_from_slice(&0x0000ff00u32.to_le_bytes());
    dds[100..104].copy_from_slice(&0x000000ffu32.to_le_bytes());
    dds[104..108].copy_from_slice(&0xff000000u32.to_le_bytes());
    dds[108..112].copy_from_slice(&0x1000u32.to_le_bytes());
    for pixel in dds[128..].chunks_mut(4) { pixel.copy_from_slice(&[10, 20, 30, 255]); }
    let input = root.join("texture.dds");
    let output = root.join("texture.out.dds");
    fs::write(&input, &dds).unwrap();
    let printed = success(&["texture-compress", "16", input.to_str().unwrap(), output.to_str().unwrap()]);
    assert!(printed.starts_with("TEXTURE_COMPRESS|"), "{printed}");
    assert_eq!(fs::read(&input).unwrap(), dds, "source must be untouched");
    assert!(fs::metadata(&output).unwrap().len() < dds.len() as u64);
    // The destination is export-only and must never be overwritten.
    assert!(!run(&["texture-compress", "16", input.to_str().unwrap(), output.to_str().unwrap()]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn content_detected_engine_audits_are_read_only_and_reject_corruption() {
    let parent = env::var_os("BGC_TEST_BTRFS_DIR").map(std::path::PathBuf::from).unwrap_or_else(env::temp_dir);
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = parent.join(format!("bgc-engine-audit-{stamp}"));
    fs::create_dir(&root).unwrap();
    // A stripped v22 metadata table with one opaque Texture2D object.
    let mut m = b"6000.0.59f2\0".to_vec();
    m.extend(19u32.to_le_bytes()); m.push(0); m.extend(1u32.to_le_bytes());
    m.extend(28u32.to_le_bytes()); m.push(0); m.extend([255; 2]); m.extend([0; 16]);
    m.extend(1u32.to_le_bytes()); while m.len() % 4 != 0 { m.push(0); }
    m.extend(1u64.to_le_bytes()); m.extend(0u64.to_le_bytes());
    m.extend(16u32.to_le_bytes()); m.extend(0u32.to_le_bytes());
    for _ in 0..3 { m.extend(0u32.to_le_bytes()); } m.push(0);
    let data = (48 + m.len()).div_ceil(16) * 16;
    let mut bytes = vec![0; 48]; bytes[8..12].copy_from_slice(&22u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&(m.len() as u32).to_be_bytes());
    bytes[24..32].copy_from_slice(&((data + 16) as u64).to_be_bytes());
    bytes[32..40].copy_from_slice(&(data as u64).to_be_bytes());
    bytes.extend(m); bytes.resize(data + 16, 0);
    let serialized = root.join("extensionless-player-data"); fs::write(&serialized, &bytes).unwrap();
    let output = success(&["container-audit", serialized.to_str().unwrap()]);
    assert!(output.contains("UNITY_SERIALIZED|22|"));
    assert!(output.contains("|little|6000.0.59f2|19|0|1|1|0"));
    assert!(output.contains("UNITY_CLASS|28|Texture2D|1|16"));
    assert_eq!(fs::read(&serialized).unwrap(), bytes);
    let xnb = root.join("page000.xnb");
    let mut xnb_bytes = b"XNBd\x05\x01".to_vec();
    xnb_bytes.extend(10u32.to_le_bytes());
    fs::write(&xnb, &xnb_bytes).unwrap();
    let output = success(&["container-audit", xnb.to_str().unwrap()]);
    assert!(output.contains("XNB_HEADER|d|5|1|none|10|1"));
    assert_eq!(fs::read(&xnb).unwrap(), xnb_bytes);
    let broken_xnb = root.join("broken.xnb");
    fs::write(&broken_xnb, b"XNBd\x05\0\x0b\0\0\0").unwrap();
    assert!(!run(&["container-audit", broken_xnb.to_str().unwrap()]).status.success());
    let link = root.join("linked.assets"); symlink(&serialized, &link).unwrap();
    assert!(!run(&["container-audit", link.to_str().unwrap()]).status.success());
    // Known SHA1 test vector "abc", not a hash computed by the reader under test.
    let mut pak = b"abc".to_vec(); pak.extend([0; 17]); pak.extend(0x5a6f12e1u32.to_le_bytes());
    pak.extend(11u32.to_le_bytes()); pak.extend(0u64.to_le_bytes()); pak.extend(3u64.to_le_bytes());
    pak.extend([0xa9,0x99,0x3e,0x36,0x47,0x06,0x81,0x6a,0xba,0x3e,
        0x25,0x71,0x78,0x50,0xc2,0x6c,0x9c,0xd0,0xd8,0x9d]);
    pak.extend([0; 160]);
    let path = root.join("sample.pak"); fs::write(&path, &pak).unwrap();
    fs::write(path.with_extension("sig"), b"presence only; never claimed verified").unwrap();
    let output = success(&["container-audit", path.to_str().unwrap()]);
    assert!(output.contains("UNREAL_INDEX|VERIFIED_PRIMARY_SHA1|3"));
    assert!(output.contains("UNREAL_SECURITY|0|1|0")); assert_eq!(fs::read(&path).unwrap(), pak);
    // The SHA1 matches, so an unparseable (here nonsensical) index body is an
    // unsupported layout, not corruption: report it without a nonzero exit.
    assert!(output.contains("UNREAL_ENTRIES|UNPARSED|"));
    pak[0] ^= 1; fs::write(&path, &pak).unwrap();
    let failed = run(&["container-audit", path.to_str().unwrap()]);
    assert!(!failed.status.success()); assert!(String::from_utf8_lossy(&failed.stderr).contains("SHA1 mismatch"));
    assert_eq!(fs::read(&path).unwrap(), pak);
    // Minimal valid IoStore TOC header plus an unparsed sibling `.ucas`.
    let mut utoc = vec![0u8; 144];
    utoc[..16].copy_from_slice(b"-==--==--==--==-"); utoc[16] = 8;
    utoc[20..24].copy_from_slice(&144u32.to_le_bytes()); utoc[32..36].copy_from_slice(&12u32.to_le_bytes());
    utoc[44..48].copy_from_slice(&0x10000u32.to_le_bytes()); utoc[52..56].copy_from_slice(&1u32.to_le_bytes());
    utoc[80] = 8;
    let toc_path = root.join("pakchunk0-Windows.utoc"); fs::write(&toc_path, &utoc).unwrap();
    fs::write(root.join("pakchunk0-Windows.ucas"), b"opaque chunk store").unwrap();
    let output = success(&["container-audit", toc_path.to_str().unwrap()]);
    assert!(output.contains("UNREAL_IOSTORE|8|144|0|0|65536|0|0|0|1|8|"));
    assert!(output.contains(&format!("UNREAL_IOSTORE_UCAS|1|{}", b"opaque chunk store".len())));
    assert!(output.contains("UNREAL_IOSTORE_SECURITY|0|0|0|1|0|0"));
    assert_eq!(fs::read(&toc_path).unwrap(), utoc);
    let cas_path = root.join("pakchunk0-Windows.ucas");
    assert_eq!(fs::read(&cas_path).unwrap(), b"opaque chunk store");
    fs::rename(&cas_path, root.join("actual.ucas")).unwrap();
    std::os::unix::fs::symlink(root.join("actual.ucas"), &cas_path).unwrap();
    assert!(success(&["container-audit", toc_path.to_str().unwrap()]).contains("UNREAL_IOSTORE_UCAS|0|0"));
    let linked_toc = root.join("linked.utoc");
    std::os::unix::fs::symlink(&toc_path, &linked_toc).unwrap();
    assert!(!run(&["container-audit", linked_toc.to_str().unwrap()]).status.success());
    let mut impossible = utoc.clone(); impossible[24..28].copy_from_slice(&1u32.to_le_bytes());
    fs::write(&toc_path, &impossible).unwrap();
    assert!(!run(&["container-audit", toc_path.to_str().unwrap()]).status.success());
    utoc[16] = 99; fs::write(&toc_path, &utoc).unwrap();
    assert!(!run(&["container-audit", toc_path.to_str().unwrap()]).status.success());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn real_btrfs_preserves_data_and_shares_blocks() {
    let Some(parent) = env::var_os("BGC_TEST_BTRFS_DIR") else {
        eprintln!("Set BGC_TEST_BTRFS_DIR for real Btrfs tests");
        return;
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = Path::new(&parent).join(format!("bgc-test-{stamp}"));
    fs::create_dir_all(base.join("game")).unwrap();
    let game = base.join("game");
    let scratch = base.join("scratch");
    let mut seed = 1u64;
    let block: Vec<u8> = (0..65536)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as u8
        })
        .collect();
    let mut data = block.repeat(8);
    data.extend_from_slice(b"unaligned EOF");
    fs::write(game.join("first"), &data).unwrap();
    fs::write(game.join("copy with spaces"), &data).unwrap();
    fs::write(game.join("empty"), []).unwrap();
    fs::write(game.join("inline"), b"short inline data").unwrap();
    fs::hard_link(game.join("first"), game.join("hardlink")).unwrap();
    fs::write(base.join("outside"), b"must remain untouched").unwrap();
    symlink(base.join("outside"), game.join("symlink")).unwrap();
    let sparse = fs::File::create(game.join("sparse")).unwrap();
    sparse.set_len(8 * 1024 * 1024).unwrap();
    let compressible = vec![b'x'; 4 * 1024 * 1024];
    fs::write(game.join("compressible"), &compressible).unwrap();
    let g = game.to_str().unwrap();
    let tmp = scratch.to_str().unwrap();
    assert!(!run(&["compress", "99", g]).status.success());
    let compression = run(&["compress", "3", g]);
    assert!(compression.status.success());
    assert!(!String::from_utf8_lossy(&compression.stderr).contains("Compressing "));
    let verbose = Command::new(env!("CARGO_BIN_EXE_bgc-native"))
        .args(["compress", "3", g])
        .env("BGC_VERBOSE", "1")
        .output()
        .unwrap();
    assert!(verbose.status.success());
    assert!(String::from_utf8_lossy(&verbose.stderr).contains("Compressing "));
    let measured = run(&["measure-bytes", g]);
    assert!(!String::from_utf8_lossy(&measured.stderr).contains("usage:"));
    // Btrfs st_blocks reports logical allocation, so verify FIEMAP's ENCODED bit.
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn ioctl(fd: i32, request: u64, ...) -> i32;
    }
    let f = fs::File::open(game.join("compressible")).unwrap();
    let mut map = [0u64; 4 + 7 * 256];
    map[1] = u64::MAX;
    map[2] = 1;
    map[3] = 256;
    assert_eq!(
        unsafe { ioctl(f.as_raw_fd(), 0xc020660b, map.as_mut_ptr()) },
        0
    );
    let count = (map[2] >> 32) as usize;
    assert!(
        (0..count).any(|i| map[4 + i * 7 + 5] & 8 != 0),
        "no compressed extents"
    );
    let before = success(&["usage", g]);
    let dedupe = run(&["dedupe", g, tmp]);
    assert!(
        dedupe.status.success(),
        "{}",
        String::from_utf8_lossy(&dedupe.stderr)
    );
    assert!(!String::from_utf8_lossy(&dedupe.stderr).contains("Hashing "));
    let out = String::from_utf8(dedupe.stdout).unwrap();
    assert!(out.contains("0 rejected"), "{out}");
    let after = success(&["usage", g]);
    let exclusive = |s: &str| s.trim().split('|').nth(1).unwrap().parse::<u64>().unwrap();
    assert!(
        exclusive(&after) < exclusive(&before),
        "{before} -> {after}"
    );
    success(&["dedupe", g, tmp]); // idempotent and accepts already-shared blocks
    assert_eq!(fs::read(game.join("first")).unwrap(), data);
    assert_eq!(fs::read(game.join("copy with spaces")).unwrap(), data);
    assert_eq!(fs::read(game.join("compressible")).unwrap(), compressible);
    assert_eq!(
        fs::read(base.join("outside")).unwrap(),
        b"must remain untouched"
    );
    assert_eq!(
        fs::metadata(game.join("first")).unwrap().ino(),
        fs::metadata(game.join("hardlink")).unwrap().ino()
    );
    assert_eq!(fs::metadata(game.join("sparse")).unwrap().blocks(), 0);
    assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
    assert!(!run(&["dedupe", g, g]).status.success());
    let symlink_root = base.join("link-to-game");
    symlink(&game, &symlink_root).unwrap();
    assert!(!run(&["compress", "3", symlink_root.to_str().unwrap()])
        .status
        .success());
    // An interrupted scan must release its scratch files and return 130.
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    fs::write(game.join("large"), block.repeat(1024)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_bgc-native"))
        .args(["dedupe", g, tmp])
        .env("BGC_VERBOSE", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    assert!(stderr.read_line(&mut line).unwrap() > 0);
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    assert_eq!(unsafe { kill(child.id() as i32, 2) }, 0);
    // Drain diagnostics so waiting never blocks on a full pipe.
    std::io::copy(&mut stderr, &mut std::io::sink()).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(130));
    assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn filesystem_space_reports_read_only_available_capacity_without_btrfs() {
    let root = env::temp_dir();
    let out = success(&["fs-space", root.to_str().unwrap()]);
    let parts: Vec<&str> = out.trim().split('|').collect();
    assert_eq!(parts.len(), 5, "{out}");
    assert_eq!(parts[0], "FS_SPACE");
    let available: u64 = parts[1].parse().unwrap();
    let free: u64 = parts[2].parse().unwrap();
    let total: u64 = parts[3].parse().unwrap();
    let fragment: u64 = parts[4].parse().unwrap();
    assert!(fragment > 0);
    assert!(available <= free && free <= total, "{out}");
    assert!(!run(&["fs-space", "/nonexistent-bgc-space-root"]).status.success());
}

#[test]
fn exact_byte_measurement_is_a_valid_command() {
    let out = run(&["measure-bytes", "/nonexistent-bgc-test-directory"]);
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("usage:"));
}

#[test]
fn visual_assets_can_be_resized_restored_and_finalized() {
    let Some(parent) = env::var_os("BGC_TEST_BTRFS_DIR") else {
        return;
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = Path::new(&parent).join(format!("bgc-assets-test-{stamp}"));
    fs::create_dir_all(&root).unwrap();
    let img = image::ImageBuffer::from_fn(1024, 1024, |x, y| {
        image::Rgb([
            ((x * 73 + y * 29 + (x ^ y) * 11) % 256) as u8,
            ((x * 17 + y * 97 + (x * y) % 251) % 256) as u8,
            ((x * 131 + y * 7 + (x + y) % 239) % 256) as u8,
        ])
    });
    let formats = [
        ("png", image::ImageFormat::Png),
        ("jpg", image::ImageFormat::Jpeg),
        ("webp", image::ImageFormat::WebP),
        ("bmp", image::ImageFormat::Bmp),
        ("tga", image::ImageFormat::Tga),
    ];
    let mut originals = Vec::new();
    for (ext, format) in formats {
        let path = root.join(format!("picture.{ext}"));
        let mut cur = std::io::Cursor::new(Vec::new());
        img.write_to(&mut cur, format).unwrap();
        let bytes = cur.into_inner();
        fs::write(&path, &bytes).unwrap();
        originals.push((path, bytes));
    }
    let g = root.to_str().unwrap();
    assert!(success(&["assets", "plan", "native", "0", g]).contains("|0|"));
    let preview = success(&["assets", "plan", "ultra-performance", "0", g]);
    assert!(preview.contains("|5|"), "{preview}");
    success(&["assets", "apply", "ultra-performance", "3", g]);
    for (path, original) in &originals {
        let resized = fs::read(path).unwrap();
        assert!(resized.len() < original.len(), "{}", path.display());
        assert_eq!(
            image::ImageReader::open(path)
                .unwrap()
                .with_guessed_format()
                .unwrap()
                .decode()
                .unwrap()
                .width(),
            640
        );
    }
    let output = success(&["assets", "restore", "native", "0", g]);
    assert!(output.contains("restore"));
    for (path, original) in &originals {
        assert_eq!(&fs::read(path).unwrap(), original);
    }
    success(&["assets", "apply", "ultra-performance", "3", g]);
    fs::write(&originals[0].0, b"changed by another process").unwrap();
    assert!(!run(&["assets", "finalize", "native", "0", g])
        .status
        .success());
    assert!(root.join(".bgc-assets-backup/picture.png").exists());
    assert!(root.join(".bgc-assets-backup/picture.jpg").exists());
    let _ = fs::write(
        &originals[0].0,
        fs::read(root.join(".bgc-assets-backup/picture.png")).unwrap(),
    );
    success(&["assets", "restore", "native", "0", g]); // interrupted-before-replace state is recoverable
    for (path, original) in &originals {
        assert_eq!(&fs::read(path).unwrap(), original);
    }
    success(&["assets", "apply", "ultra-performance", "3", g]);
    success(&["assets", "finalize", "native", "0", g]);
    assert!(!root.join(".bgc-assets-backup").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unused_variants_and_debug_symbols_can_be_pruned_restored_and_finalized() {
    let Some(parent) = env::var_os("BGC_TEST_BTRFS_DIR") else { return; };
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = Path::new(&parent).join(format!("bgc-prune-test-{stamp}"));
    fs::create_dir_all(root.join("Content/Movies/720p")).unwrap();
    fs::create_dir_all(root.join("Content/Win/Packages/BC3")).unwrap();
    let movie = root.join("Content/Movies/720p/scene.bik");
    let package = root.join("Content/Win/Packages/BC3/Textures.pkg");
    let symbols = root.join("EngineWin64sv.pdb");
    fs::write(&movie, vec![7u8; 4096]).unwrap();
    fs::write(&package, vec![9u8; 8192]).unwrap();
    fs::write(&symbols, vec![3u8; 2048]).unwrap();
    fs::write(root.join("Content/Movies/full.bik"), b"kept full-resolution video").unwrap();
    let g = root.to_str().unwrap();
    let plan = success(&["prune-plan", g, "--debug", "Content/Movies/720p", "Content/Win/Packages/BC3"]);
    assert_eq!(plan.trim(), "PRUNE|plan|3|14336");
    success(&["prune-apply", g, "--debug", "Content/Movies/720p", "Content/Win/Packages/BC3"]);
    for path in [&movie, &package, &symbols] {
        assert!(!path.exists(), "{} still present", path.display());
    }
    assert_eq!(fs::read(root.join("Content/Movies/full.bik")).unwrap(), b"kept full-resolution video");
    // The backup copy plus the removal marker make the state known and restorable.
    assert_eq!(fs::read(root.join(".bgc-assets-backup/EngineWin64sv.pdb")).unwrap(), vec![3u8; 2048]);
    success(&["assets", "restore", "native", "0", g]);
    assert_eq!(fs::read(&movie).unwrap(), vec![7u8; 4096]);
    assert_eq!(fs::read(&package).unwrap(), vec![9u8; 8192]);
    assert_eq!(fs::read(&symbols).unwrap(), vec![3u8; 2048]);
    // Re-apply, then finalize: the backup tree disappears and files stay removed.
    success(&["prune-apply", g, "--debug", "Content/Movies/720p", "Content/Win/Packages/BC3"]);
    success(&["assets", "finalize", "native", "0", g]);
    assert!(!root.join(".bgc-assets-backup").exists());
    assert!(!movie.exists() && !package.exists() && !symbols.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn hades_packages_losslessly_recompress_restore_and_finalize() {
    let Some(parent) = env::var_os("BGC_TEST_BTRFS_DIR") else { return; };
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root = Path::new(&parent).join(format!("bgc-pkg-test-{stamp}"));
    fs::create_dir(&root).unwrap();
    let mut seed = 12345u32;
    let noise: Vec<u8> = (0..65536).map(|_| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as u8
    }).collect();
    let mut payload = Vec::new();
    for offset in 0..512 {
        payload.extend_from_slice(&noise[offset..offset+4096]);
    }
    let compressed = lz4::block::compress(&payload, Some(lz4::block::CompressionMode::FAST(1)), false).unwrap();
    let mut original = vec![0x20, 0, 0, 7, 1];
    original.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    original.extend_from_slice(&compressed);
    let package = root.join("Textures.pkg");
    fs::write(&package, &original).unwrap();
    fs::write(root.join("Textures.pkg_manifest"), b"unchanged atlas coordinates").unwrap();
    let image_path = root.join("large.bmp");
    image::RgbImage::new(2048, 16).save(&image_path).unwrap();
    let image_original = fs::read(&image_path).unwrap();
    let g = root.to_str().unwrap();
    let preview = success(&["assets", "plan", "lossless", "0", g]);
    assert!(preview.contains("|1|"), "{preview}");
    success(&["assets", "apply", "lossless", "3", g]);
    let optimized = fs::read(&package).unwrap();
    assert!(optimized.len() < original.len());
    assert_eq!(lz4::block::decompress(&optimized[9..], Some(0x2000000-4)).unwrap(), payload);
    assert_eq!(fs::read(&image_path).unwrap(), image_original);
    assert_eq!(fs::read(root.join("Textures.pkg_manifest")).unwrap(), b"unchanged atlas coordinates");
    assert_eq!(fs::read(root.join(".bgc-assets-backup/Textures.pkg")).unwrap(), original);
    success(&["assets-restore-file", g, "Textures.pkg"]);
    assert_eq!(fs::read(&package).unwrap(),original);
    success(&["assets", "apply", "lossless", "3", g]);
    success(&["assets", "restore", "native", "0", g]);
    assert_eq!(fs::read(&package).unwrap(), original);
    success(&["assets", "apply", "lossless", "3", g]);
    success(&["assets", "finalize", "native", "0", g]);
    assert!(!root.join(".bgc-assets-backup").exists());
    assert_eq!(fs::read(&package).unwrap(), optimized);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn texture_guards_are_automatic_in_the_main_pipeline_for_every_profile() {
    let Some(parent) = env::var_os("BGC_TEST_BTRFS_DIR") else { return; };
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let base = Path::new(&parent).join(format!("bgc-policy-test-{stamp}"));
    fs::create_dir(&base).unwrap();
    let large = image::RgbImage::from_fn(2048, 1024, |x, y| {
        image::Rgb([(x * 73 + y * 29) as u8, (x * 17 + y * 97) as u8, (x * 131 + y * 7) as u8])
    });
    for (profile, expected_width) in [
        ("native", 2048), ("lossless", 2048),
        ("ultra-performance", 1024), ("performance", 1280),
        ("balanced", 1920), ("quality", 2048), ("ultra-quality", 2048),
    ] {
        let root = base.join(profile);
        fs::create_dir(&root).unwrap();
        let large_path = root.join("large.bmp");
        large.save(&large_path).unwrap();
        let original_large = fs::read(&large_path).unwrap();
        let mut protected = Vec::new();
        for (name, w, h) in [("small.bmp", 256, 256), ("thin.bmp", 2048, 64), ("ui_atlas.bmp", 2048, 1024)] {
            let path = root.join(name);
            image::RgbImage::new(w, h).save(&path).unwrap();
            protected.push((path.clone(), fs::read(&path).unwrap()));
        }
        let unknown = root.join("sharedassets0.assets");
        fs::write(&unknown, b"unknown serialized Unity resource; never reinterpret as loose art").unwrap();
        protected.push((unknown.clone(), fs::read(&unknown).unwrap()));
        let g = root.to_str().unwrap();
        success(&["assets", "apply", profile, "3", g]);
        assert_eq!(image::ImageReader::open(&large_path).unwrap().decode().unwrap().width(), expected_width, "{profile}");
        for (path, bytes) in &protected { assert_eq!(&fs::read(path).unwrap(), bytes, "{profile}: {}", path.display()); }
        let after = fs::read(&large_path).unwrap();
        success(&["assets", "apply", profile, "3", g]);
        assert_eq!(fs::read(&large_path).unwrap(), after, "{profile}: repeated apply changed texture");
        if expected_width == 2048 {
            assert_eq!(after, original_large);
        } else {
            assert!(after.len() < original_large.len());
            success(&["assets", "restore", "native", "0", g]);
            assert_eq!(fs::read(&large_path).unwrap(), original_large);
        }
    }
    fs::remove_dir_all(base).unwrap();
}
