# Steam library asset coverage snapshot — 2026-10-04

The installed library discovery snapshot contains 316 Steam entries. This is a
format coverage report, not a promise that every title can use lossy assets.
The volume is Btrfs with about 159 GB free at the time of this run, so whole
library copies were not attempted.

## Existing Balanced candidate planner

The earlier decoder-backed `assets plan balanced 0` scan finished for 316 entries. It
reported supported candidates in 66 installs; seven very large installs hit
the 180-second per-install scan ceiling. A fast metadata-only planner mode is now
available to inventory supported loose formats across the current 310 installs;
it deliberately does not estimate candidates for packed containers. Candidates
include loose raster images, simple WAVs, bounded standalone FMOD banks, Hades
packages, and Godot PCKs. They
are logical candidate bytes, not measured disk savings or proof of playability.
The scan report is retained outside the repository at
`/tmp/opencode/bgc-steam-balanced-asset-plans.json`.

The earlier 310-row planning sweep is not a valid candidate census: the backend
was rebuilt during the run, mixing decoder-backed and metadata-only responses.
Its two nonzero rows (BALLxPIT and Blasphemous) do not establish that the other
308 games have no candidates. FMOD bank candidates are standalone audio, not
proof of packed Unity rewrite support.

A subsequent clean `--assets-all` run using the validated inventory response
format completed all 310 installs with zero errors
(`ASSET_INVENTORY_SUMMARY|310|310|0`). This counts file metadata only and makes no
savings or compatibility claim. Decoder-backed planning must be rerun against a
fixed backend build before its totals can be used.

## Standalone containers

- Godot PCK: 16 game installs identified in the read-only container inventory.
  Nine were tested on disposable copies through the full Balanced apply path,
  post-audited, run a second time, and verified against the unchanged installed
  source. Four shrank in the original seven-pack trial; the follow-up copy batch
  additionally confirmed Confidential Killings. See the PCK trial report and
  `godot-followup-copies-2026-10-03.json`.
- FMOD `.bank`/`.fsb`: 59 installs, 1,444 files and about 27.9 GiB by extension
  inventory. The strict read-only parser accepted 889; 555 were rejected as
  unsupported layouts/metadata banks. Eight full disposable-game-copy trials
  passed post-audit and restore-to-hash checks. This does not replace runtime
  audio/playback validation.
- Hades `.pkg`: identified by extension where present, but the broad scan does
  not infer its format version or claim every package is eligible. Existing
  package parser, physical acceptance gate and restore path remain authoritative.

## Unsupported coverage

Unity serialized textures in sampled installed games have stripped player type
trees, so the current code can inventory them but not rewrite them safely.
Unreal Pak/IoStore, Wwise, CRI, Bink/video, arbitrary XNB and embedded FMOD remain
read-only or untouched. A parser rejection is not permission to guess a layout.

No installed game was changed during this coverage work. All transform trials
used disposable copies, were removed after validation, and preserved source
hashes. No visual/playback test was run for the new trials.
