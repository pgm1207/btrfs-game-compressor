# Targeted Balanced asset opportunity scan — 2026-10-07

Read-only scan of three installed Steam games with the existing `bgc-native`
binary, SHA-256 `91cf95a4af520a5afc4a27574dc5aee6876fda001f26076341cdef02e39e549f`.
The saved `--status --json` source listed 311 games, but only these three were
selected. The new `tools/asset-opportunity-report.py` pinned that backend and
ran `asset-plan balanced` plus separate Godot 4 PCK texture audits. It wrote
its full JSON/Markdown report outside the repository under
`/tmp/bgc-targeted-opportunities-2026-10-07/`; no installed game was changed.

| Game | Planner reduction | Godot 4 PCK reduction | Combined logical estimate | Scan time |
|---|---:|---:|---:|---:|
| Slay the Spire 2 | 0 | 424,300,096 bytes | 424,300,096 bytes | 91.5 s |
| Pathogenic | 0 | 390,163,536 bytes | 390,163,536 bytes | 65.2 s |
| Hohokum | 142,344,324 bytes | 0 | 142,344,324 bytes | 40.9 s |

The combined total is **956,807,956 logical bytes** across the three selected
games. The ordinary planner counts Godot PCK files but assigns them no predicted
reduction; the Godot audit estimates are separate and are added exactly once.
Hohokum's three planner candidates total 201,326,976 source bytes. Its three
largest loose DDS files are each 67,108,992 bytes, matching that total; this is
strong evidence that the Balanced candidates are those DDS files, though the
planner does not emit per-file candidate identities.

These are **not measured physical Btrfs savings**. The report does not establish
visual quality or runtime compatibility. In particular, a previous Pathogenic
candidate [failed to launch](pathogenic-ultra-2026-10-02.md#runtime-compatibility-failure--candidate-on-hold),
so its estimate must not be treated as a playable build. Slay and Hohokum were
not launched in this scan. No copy/export/apply or privileged extent measurement
was performed.
