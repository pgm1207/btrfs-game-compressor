# Filesystem-neutral asset writer: native-only incremental capability

This change is a **backend milestone**, not a claim that the entire
Btrfs Game Compressor CLI now works on ext4 Steam Deck libraries.

The native command can apply a supported loose-media transform with a
descriptor-confined Linux file open, even if Btrfs defragment-compression ioctls
are unavailable on the target filesystem:

```sh
bgc-native assets plan ultra-performance 0 /path/to/disposable/game-copy
bgc-native assets apply ultra-performance 1 /path/to/disposable/game-copy
bgc-native assets restore native 0 /path/to/disposable/game-copy
```

The supported codecs, source-format guards and strict savings gates are unchanged.
On Btrfs, the rewritten file still receives Zstd compression when available;
on ext4 and other non-Btrfs filesystems the codec output is installed without
attempting to emulate filesystem compression. Ordinary backups may need a
full physical copy instead of a Btrfs reflink and thus consume substantially
more disk space until finalized. Use only disposable test game copies until the
full recovery and game-playability gates have passed.

**Not yet included:**

- Steam-library discovery and normal shell TUI apply on ext4 (they still
  require a Btrfs library); no automatic filesystem conversion.
- Filesystem-level Btrfs Zstd compression or extent deduplication on ext4.
- A general-purpose guarantee for arbitrary other filesystems, external mounts,
  unknown pack formats, or concurrent directory mutations.
- UI packaging, end-to-end interrupted-write recovery, and game runtime testing.

The generic root still uses Linux `openat2` confinement and rejects symlink
traversal for the loose file write path. The scoped implementation deliberately
leaves compression and deduplication commands gated on Btrfs.
