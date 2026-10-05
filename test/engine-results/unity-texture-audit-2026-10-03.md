# Unity installed-game audit sample — 2026-10-03

Read-only `bgc-native container-audit` trials against installed Steam files. No
game was launched, no assets were written, and no stream paths were followed.
Only bounded metadata and eligible Texture2D object records were read.

| Game | SerializedFile | Engine build | Format | Trees | Texture2D objects | Texture details |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| Pony Island | `sharedassets0.assets` | 2017.4.40f1 | 17 | no | 25 | 0 |
| Outer Wilds | `sharedassets0.assets` | 2019.4.39f1 | 21 | no | 357 | 0 |
| Dorfromantik | `sharedassets0.assets` | 2021.3.45f2 | 22 | no | 572 | 0 |
| A Short Hike | `sharedassets0.assets` | 2021.3.45f2 | 22 | no | 7 | 0 |
| Mech Havoc | `sharedassets0.assets` | 6000.0.59f2 | 22 | no | 506 | 0 |

All five returned success and parsed their SerializedFile metadata/class tables;
none contains player type trees, so none exercises the TextureFormat name mapping
on real texture metadata. Bounded synthetic type-tree tests exercise the metadata
walker and format-name mapping. These results do not establish codec payload
validity, stream extent validity, visual quality, or writer compatibility.
Unknown/stripped texture records correctly remain opaque.
