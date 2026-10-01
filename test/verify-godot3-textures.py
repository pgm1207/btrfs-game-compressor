#!/usr/bin/env python3
"""Independent GDST candidate check; requires Pillow for image decoding.

SOURCE is the PCM-only pack. Only .stex entries may differ.
Checks logical dimensions, flags, encoded payload size, RGB quantization and
unchanged alpha for non-resized textures. Does not prove in-game atlas behavior.
"""
from io import BytesIO
from pathlib import Path
import runpy
import struct
import sys
from PIL import Image

pack = runpy.run_path(str(Path(__file__).with_name('verify-godot3-samples.py')))['pack']


def texture(data):
    assert data[:4] == b'GDST' and len(data) >= 28
    w, custom_w, h, custom_h, flags, fmt, levels, size = struct.unpack_from('<4H4I', data, 4)
    assert levels == 1 and size == len(data) - 28
    payload = data[28:]
    if fmt & (1 << 21):
        assert payload[:4] == b'WEBP'
        payload = payload[4:]
    else:
        assert fmt & (1 << 20)
    image = Image.open(BytesIO(payload)).convert('RGBA')
    assert image.size == (w, h)
    return (custom_w or w, custom_h or h), flags, fmt, image


def main():
    source, candidate = map(pack, sys.argv[1:3])
    max_edge = int(sys.argv[3]) if len(sys.argv) > 3 else 854
    color_bits = int(sys.argv[4]) if len(sys.argv) > 4 else 5
    assert source.keys() == candidate.keys()
    changed = resized = saved = 0
    levels = (1 << color_bits) - 1
    allowed = {round(i * 255 / levels) for i in range(levels + 1)}
    for name, old in source.items():
        new = candidate[name]
        if old == new:
            continue
        assert name.endswith('.stex'), 'unexpected changed asset: ' + name
        dims_a, flags_a, fmt_a, a = texture(old)
        dims_b, flags_b, fmt_b, b = texture(new)
        assert dims_a == dims_b and flags_a == flags_b and fmt_a == fmt_b
        assert len(new) < len(old)
        assert max(b.size) <= max_edge or b.size == a.size
        for channel in b.split()[:3]:
            assert set(channel.tobytes()).issubset(allowed)
        if a.size == b.size:
            assert a.getchannel('A').tobytes() == b.getchannel('A').tobytes()
        else:
            resized += 1
        changed += 1
        saved += len(old) - len(new)
    assert changed > 0
    print(f'PASS: {changed} textures ({resized} resized); {saved:,} resource bytes saved; '
          'logical sizes, flags, MD5s and unchanged assets verified.')


if __name__ == '__main__':
    main()
