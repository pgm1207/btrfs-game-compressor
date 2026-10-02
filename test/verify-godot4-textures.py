#!/usr/bin/env python3
"""Independent streaming PCK v3/v4 + GST2 texture candidate verification.

Usage: verify-godot4-textures.py SOURCE CANDIDATE [MAX_EDGE] [--conservative]
Pillow decodes PNG/WebP and BC1/2/3/7 via temporary in-memory DDS headers.
Checksums/structure are checked, not perceptual fidelity or game compatibility.
"""
import hashlib
from io import BytesIO
import math
import struct
import sys
from PIL import Image


def number(f, fmt):
    data = f.read(struct.calcsize('<' + fmt))
    assert len(data) == struct.calcsize('<' + fmt)
    return struct.unpack('<' + fmt, data)[0]


def directory(f):
    f.seek(0)
    assert f.read(4) == b'GDPC'
    version = number(f, 'I')
    assert version in (3, 4)
    engine = tuple(number(f, 'I') for _ in range(3))
    assert number(f, 'I') & ~2 == 0
    base, offset = number(f, 'Q'), number(f, 'Q')
    f.seek(0, 2); length = f.tell()
    assert base < offset < length
    f.seek(offset)
    count = number(f, 'I'); assert count <= 200000
    out = {}
    for _ in range(count):
        n = number(f, 'I'); assert 0 < n <= 4096
        name = f.read(n).rstrip(b'\0').decode('utf-8')
        start, size = number(f, 'Q') + base, number(f, 'Q')
        digest = f.read(16)
        assert number(f, 'I') == 0
        assert start + size <= length and name not in out
        out[name] = (start, size, digest)
    return version, engine, out


def hash_entry(f, entry):
    start, size, expected = entry
    f.seek(start); digest = hashlib.md5()
    while size:
        data = f.read(min(size, 1048576)); assert data
        digest.update(data); size -= len(data)
    assert digest.digest() == expected


def bytes_at(f, entry):
    f.seek(entry[0]); data = f.read(entry[1]); assert len(data) == entry[1]
    return data


def ctex_header(data):
    assert len(data) >= 52 and data[:4] == b'GST2'
    w, h, mips, fmt = struct.unpack_from('<HHII', data, 40)
    encoding = struct.unpack_from('<I', data, 36)[0]
    assert w and h and mips <= 16 and encoding <= 2
    return w, h, mips, fmt, encoding


def decode_ctex(data):
    w, h, mips, fmt, encoding = ctex_header(data)
    if encoding in (1, 2):
        pos = 52; base = None
        for i in range(mips + 1):
            size = struct.unpack_from('<I', data, pos)[0]; pos += 4
            payload = data[pos:pos + size]; assert len(payload) == size
            image = Image.open(BytesIO(payload)).convert('RGBA')
            assert image.size == (max(1, w >> i), max(1, h >> i))
            if i == 0:
                base = image
            pos += size
        assert pos == len(data)
        return base
    assert fmt in (17, 18, 19, 22)
    block = 8 if fmt == 17 else 16
    expected = sum(((max(1, w >> i) + 3) // 4) * ((max(1, h >> i) + 3) // 4) * block
                   for i in range(mips + 1))
    assert len(data) == 52 + expected
    dxgi = {17: 71, 18: 74, 19: 77, 22: 98}[fmt]
    # DDS header plus DX10 extension; no filesystem side effects.
    header = struct.pack('<7I', 124, 0xA1007, h, w, ((w+3)//4)*((h+3)//4)*block, 0, mips+1)
    header += bytes(44)
    header += struct.pack('<II4s5I', 32, 4, b'DX10', 0, 0, 0, 0, 0)
    header += struct.pack('<5I', 0x401008 if mips else 0x1000, 0, 0, 0, 0)
    header += struct.pack('<5I', dxgi, 3, 0, 1, 0)
    image = Image.open(BytesIO(b'DDS ' + header + data[52:])).convert('RGBA')
    assert image.size == (w, h)
    return image


def main():
    max_edge = int(sys.argv[3]) if len(sys.argv) > 3 else 640
    conservative = '--conservative' in sys.argv[4:]
    changed = saved = 0
    with open(sys.argv[1], 'rb') as a, open(sys.argv[2], 'rb') as b:
        va, ea, source = directory(a); vb, eb, candidate = directory(b)
        assert va == vb and ea == eb and source.keys() == candidate.keys()
        for name, old in source.items():
            new = candidate[name]
            hash_entry(a, old); hash_entry(b, new)
            if old[1:] == new[1:]:
                a.seek(old[0]); b.seek(new[0]); left = old[1]
                while left:
                    n = min(left, 1048576)
                    assert a.read(n) == b.read(n), name
                    left -= n
                continue
            assert name.endswith('.ctex') and new[1] < old[1], name
            original, output = bytes_at(a, old), bytes_at(b, new)
            before, after = ctex_header(original), ctex_header(output)
            assert original[:40] == output[:40], 'logical dimensions/flags changed: ' + name
            assert before[3:] == after[3:], 'codec changed: ' + name
            w, h, mips, fmt, encoding = after
            allowed_edge = max_edge
            if conservative:
                logical_w, logical_h = struct.unpack_from('<II', original, 8)
                assert logical_w and logical_h, 'missing original-size reference: ' + name
                reference_w = max(logical_w, before[0])
                reference_h = max(logical_h, before[1])
                allowed_edge = max(max_edge, (max(reference_w, reference_h) + 1) // 2)
                assert w * 2 + 1 >= reference_w and h * 2 + 1 >= reference_h, name
                assert max(before[:2]) > 512 and min(before[:2]) > 64, name
                assert min(w, h) >= 64, name
            assert max(w, h) <= allowed_edge and w <= before[0] and h <= before[1]
            assert mips == (int(math.log2(max(w, h))) if before[2] else 0)
            image = decode_ctex(output)
            if encoding in (1, 2) and before[:2] == after[:2]:
                original_image = decode_ctex(original)
                assert original_image.getchannel('A').tobytes() == image.getchannel('A').tobytes()
            changed += 1; saved += old[1] - new[1]
    assert changed > 0
    print(f'PASS: PCK v{va}, {changed} textures, {saved:,} payload bytes saved; '
          'all hashes, unchanged assets, logical sizes, codecs and new mip chains verified.')


if __name__ == '__main__':
    main()
