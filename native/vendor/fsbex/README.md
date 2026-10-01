# FMOD Vorbis setup lookup

`vorbis_lookup.rs` is imported unchanged from the MIT-licensed `fsbex` 0.3.0
crate (https://github.com/astral4/fsbex). Its full MIT notice is in `LICENSE-MIT`.
The source table credits vgmstream and Fmod5Sharp through upstream's README.

This table maps the FSB5 CRC identifiers to Vorbis setup headers. The native
FMOD exporter uses it for direct PCM decoding and verifies generated setup
headers against it before exporting. No FMOD SDK or proprietary encoder is
bundled or invoked. Having a matching codebook is not, by itself, proof of
in-game bank compatibility; exported files still require playback/loop tests.
