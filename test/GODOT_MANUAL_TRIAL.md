# Manual Godot trial: Slay the Spire 2

Initial Godot support is explicit and experimental. `--apply-assets` does not
rewrite PCK packs. Native/Lossless never reduce texture or audio quality.
Godot 4 audio/FMOD banks and fonts remain unchanged; unsupported textures are
copied unchanged. Ultra Performance caps the physical texture edge at 640 px
and reduces RGB precision. This does not set the game's display resolution.

Run from the repository directory with Steam updates/downloads and the game
closed. Build the native backend first (`make native`; Rust 1.89+ required).
The following creates a new candidate; it does **not** install it:

```bash
game='/mnt/storage/Games/SteamLibrary/steamapps/common/Slay the Spire 2'
trial='/mnt/storage/Games/bgc-slay-manual'
mkdir -p "$trial"
./btrfs-game-compressor --export-godot-textures ultra-performance \
  "$game/SlayTheSpire2.pck" "$trial/SlayTheSpire2.pck"
```

An existing output is never overwritten. Stop if the command reports no export
or an error. Optionally verify independently (Python 3 and Pillow required):

```bash
python3 test/verify-godot4-textures.py \
  "$game/SlayTheSpire2.pck" "$trial/SlayTheSpire2.pck" 640
```

Apply filesystem compression and deduplication to the candidate only, in this
order. The filesystem must be Btrfs; scratch must be outside the scanned tree:

```bash
./bgc-native compress 1 "$trial"
./bgc-native dedupe "$trial" /mnt/storage/Games/bgc-slay-dedupe-scratch
```

The candidate remains separate from the installed original. For a reversible
manual playtest, set Steam's game launch options to:

```text
--main-pack /mnt/storage/Games/bgc-slay-manual/SlayTheSpire2.pck
```

Launch the game yourself. Check menus, combat, text, animations and audio. If
the game's launcher does not pass this Godot option through, do not assume it
loaded the candidate; keep the original intact and investigate the launcher.
Remove the launch option to return to the original pack. Never delete the
candidate while the game is using it. Once the game is closed and the launch
option removed, this newly created trial directory can be cleaned up.

This is not net freed disk space: the original and candidate coexist. Earlier
measurements are in `engine-results/godot-library-2026-10-01.md`; they describe
an earlier installed trial, not the current reinstalled game.
