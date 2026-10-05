# Carrion Steam trial — 2026-10-03

All trials used `/home/pgm1207/.local/share/Steam/steamapps/common/Carrion`.
The installed tree was only read; no game process was launched. Exports ran to an
automatically cleaned temporary directory under `/tmp/opencode`.

## Container inventory and XNB audit

`engine-scan` reports `unknown` for the game executable/layout, and inventories
3 FMOD-named files totaling 98,857,230 bytes plus 193 signature-validated XNB
files totaling 137,994,632 bytes. It deliberately does not call the game
XNA/MonoGame/FNA: the XNB files may be bundled viewer/art-book content, not proof
of the runtime engine.

Both `CarrionComicBook/Content/page000.xnb` and
`CarrionArtBook/Content/page000.xnb` pass `container-audit`:

```text
XNB_HEADER|d|5|0|none|4194389|0
```

This validates the target, version, flags and exact header file-size declaration
only. Reader IDs, decompressed content, texture surfaces and game playback remain
uninspected.

## FMOD bank checks

The two large installed RIFF/FEV banks pass both `container-audit` and the
bounded offline decoder benchmark:

| Bank | File bytes | Sampled streams | Sampled frames | Decoded audio |
| --- | ---: | ---: | ---: | ---: |
| `Content/Audio/MasterBank.bank` | 52,613,600 | 7 | 1,682,816 | 35.058667 s |
| `Content/Audio/Sounds.bank` | 46,213,120 | 8 | 567,040 | 11.813333 s |

A balanced export of `Sounds.bank` to a disposable output reported `EXPORTED`,
from 46,213,120 to 45,819,328 bytes. The exported bank then passed the generic
`container-audit` FMOD route and the same decoder benchmark. The source SHA-256
before and after was identical. This is a technical round-trip/audit result, not
a listening or Carrion runtime test; do not replace the installed bank based on
this trial alone. A debug-build export attempt on the larger master bank was
stopped at its 180-second timeout; only its read-only audit is counted here.

## Changes prompted by the trial

- `container-audit` now recognizes bounded XNB v4–6 headers and reports target,
  version, flags, compression flag and declared size; it does not parse payloads.
- `engine-scan` inventories `.xnb` only after signature/header validation and
  does not infer an engine from XNB presence.
- `container-audit` routes FSB5 and RIFF/FEV signatures to the existing bounded
  FMOD audit instead of attempting an Unreal Pak parse.
- Synthetic malformed-header tests and a filesystem CLI/read-only test pass.

No savings, compatibility, texture-quality, audio-perceptual-quality or in-game
performance claim is made.
