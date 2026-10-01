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
            854
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
