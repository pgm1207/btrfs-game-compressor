# Godot Balanced (1080p) disposable Steam-copy trial — 2026-10-03

All source packs were copied from the installed Steam library to a temporary
directory on the same Btrfs filesystem. Each copy was audited before and after,
then passed through the real `assets apply balanced 1` path twice. A second pass
had to retain identical size and SHA-256. Temporary copies were removed; no
installed game files were changed. This is format-level verification, not
in-game compatibility testing.

| Game | PCK | Before | After | Result |
| --- | --- | ---: | ---: | --- |
| Dungeons & Degenerate Gamblers | v3 / Godot 4.5.1 | 621,545,696 | 621,545,696 | no eligible shrink |
| Click the Button | v4 / Godot 4.7.1 | 54,558,932 | 54,538,596 | 20,336 bytes smaller |
| FEED THE QUEEN | v3 / Godot 4.5.1 | 101,220,764 | 94,797,116 | 6,423,648 bytes smaller |
| Idols of Ash | v3 / Godot 4.6.3 | 82,096,444 | 82,096,444 | no eligible shrink |
| Confidential Killings | v3 / Godot 4.6.3 | 371,494,880 | 346,158,064 | 25,336,816 bytes smaller |
| Lost Wiki: Kozlovka | v2 / Godot 4.4.1 | 46,412,992 | 46,412,992 | unchanged; unsupported writer layout |
| Project P.I.T.T. | v4 / Godot 4.7.2 | 96,487,884 | 96,108,924 | 378,960 bytes smaller |
| Skeleseller | v3 / Godot 4.6.1 | 114,618,620 | 114,618,620 | no eligible shrink |
| Brotato | v1 / Godot 3.7.0 | 147,912,168 | 99,789,023 | 48,123,145 bytes smaller |

All copied inputs matched their installed source SHA-256 before transformation.
Every post-transform pack passed the existing read-only PCK audit. Every second
apply was a byte-identical no-op. Output writers verify PCK metadata, recomputed
payload checksums and unchanged entries. Packed Godot transformations do not
retain an app-local recovery copy; Steam verification remains the recovery path.

The result confirms the 1080p path across PCK v1/v3/v4 inputs with eligible
assets. Additional disposable-copy trials on Dungeons & Degenerate Gamblers,
Idols of Ash, and Lost Wiki: Kozlovka also passed PCK audits and second-pass
idempotence checks; only the Lost Wiki pack was PCK v2. Confidential Killings
also passed its copy, post-audit, and idempotence checks and shrank by 25,336,816
bytes. PCK v2 is inventoried but not rewritten. Games with no beneficial
supported texture/audio rewrite remain unchanged. It does not establish that
every game in the library is Godot or that rewritten games load correctly.

Read-only audit coverage on 2026-10-04 also passed Until Then's 211,483-entry
PCK v2 directory after increasing the bounded large-pack directory budget.
Gamblers Table's PCK v3 layout remains rejected because it declares an
unsupported encrypted/sparse flag combination. The audit reads bounded metadata;
it does not hash every multi-gigabyte payload or certify runtime loading.
