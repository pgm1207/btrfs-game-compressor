# Brotato WAV stage — corrected playback layout

2026-10-01. Source: manually playtested MP3/Vorbis pack, 88,554,018 bytes.
Candidate: `Brotato.wav-checked-v2.pck`, 82,819,556 bytes.

- Converted 81 of 168 `.sample` resources to Godot 3 IMA-ADPCM.
- 87 unchanged: unsupported formats, odd frame counts, or failing a per-channel
  20 dB SNR gate. Odd lengths are skipped to preserve duration exactly.
- Incremental logical saving: 5,734,462 bytes (5.47 MiB). This supersedes the
  unverified 13,554,205-byte WAV estimate; it is not a physical-storage result.
- Preserved rate, stereo, loop metadata and exact decoded frame count.
- Reverse/ping-pong loops are skipped because Godot's IMA playback forces
  forward loops. In-game looping remains a manual validation requirement.

The earlier implementation incorrectly assumed WAV/miniaudio headers. Godot
3's `scene/resources/audio_stream_sample.cpp` instead starts decoder state at
zero and reads headerless, low-nibble-first packed bytes. Stereo interleaves
one byte per channel for each pair of frames. The reference importer emits
zero header bytes, but playback treats those as audio, not metadata; this
exporter omits them to avoid adding silent frames.

Checks: 73 unit tests passed. Independent Python verification using a separate
RSRC parser checked every pack-entry MD5, unchanged non-sample entries, metadata,
frame counts and per-channel fidelity for all 81 converted resources. Minimum
per-channel SNR: 20.10 dB. Rust verification minimum combined SNR: 20.57 dB.
SNR is an engineering guard, not proof that lossy sounds are perceptually equal.

Installed reversibly after Zstd level 1 recompression of the staged pack.
Pre-WAV backup: `/mnt/storage/Games/bgc-test2/Brotato.pck.pre-wav-v2`.
Original unmodified game pack backup remains `Brotato.pck.install-original`.
No textures, fonts or executables were changed. Physical savings are unmeasured.
User manual playtest FAILED: Brotato crashes when entering the main menu.
The exact pre-WAV pack was restored (88,554,018 bytes, SHA256
`a5bba55d21c27f03452dc3753a054d48d98c08f12242841db8b5d9f6e1320c30`).
IMA conversion is disabled in the exporter. The root cause is not established;
passing offline checks was insufficient to establish engine compatibility.
Do not install the WAV candidate. Future research should retain PCM encoding
and consider lower sample rates or bit depth, with no installed changes until
explicitly approved and tested.

Independent check:

```sh
python3 test/verify-godot3-samples.py \
  /mnt/storage/Games/bgc-test2/Brotato.pck.pre-wav-v2 \
  '/mnt/storage/Games/SteamLibrary/steamapps/common/Brotato/Brotato.pck'
```
