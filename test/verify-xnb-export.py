#!/usr/bin/env python3
"""Independently check a detached, single-mip XNB Texture2D export."""

import argparse
import hashlib
from io import BytesIO
import json
import math
import os
from pathlib import Path
import re
import stat
import struct

MAX_FILE_BYTES = 64 * 1024 * 1024
MAX_PIXELS = 16_777_216


def read_snapshot(path):
    """Bounded regular-file read; hashes describe this snapshot, not an install."""
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    before = os.fstat(fd)
    if not stat.S_ISREG(before.st_mode) or not 10 <= before.st_size <= MAX_FILE_BYTES:
        # Validate on the raw descriptor before wrapping it: fdopen refuses a
        # directory with IsADirectoryError, which is a different failure surface.
        os.close(fd)
        raise ValueError('XNB verification requires a bounded regular file')
    with os.fdopen(fd, 'rb', buffering=0) as source:
        chunks = []
        remaining = before.st_size
        while remaining:
            chunk = source.read(min(remaining, 1024 * 1024))
            if not chunk:
                raise ValueError('XNB input truncated during verification')
            chunks.append(chunk)
            remaining -= len(chunk)
        after = os.fstat(source.fileno())
        if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (
                after.st_size, after.st_mtime_ns, after.st_ctime_ns):
            raise ValueError('XNB input changed during verification')
        return b''.join(chunks)


def known_texture_reader(name, version):
    """Independent, non-executing implementation of the reader-name subset."""
    parts = [part.strip() for part in name.split(',')]
    if version != 0 or parts[0] != 'Microsoft.Xna.Framework.Content.Texture2DReader':
        return False
    if len(parts) == 1:
        return True
    if parts[1] not in ('Microsoft.Xna.Framework', 'Microsoft.Xna.Framework.Graphics',
                        'MonoGame.Framework', 'FNA'):
        return False
    seen = set()
    for qualifier in parts[2:]:
        key, separator, value = qualifier.partition('=')
        if not separator or key in seen:
            return False
        seen.add(key)
        if key == 'Version':
            components = value.split('.')
            if (len(components) != 4 or any(not re.fullmatch(r'[0-9]{1,5}', part)
                                           or int(part) > 65535 for part in components)):
                return False
        elif key == 'Culture':
            if value != 'neutral':
                return False
        elif key == 'PublicKeyToken':
            if value != 'null' and not re.fullmatch(r'[0-9a-fA-F]{16}', value):
                return False
        else:
            return False
    return True


def read_7bit(data, offset):
    value = 0
    for step in range(5):
        if not 0 <= offset < len(data):
            raise ValueError('truncated 7-bit value')
        byte = data[offset]
        offset += 1
        if step == 4 and byte > 7:
            raise ValueError('invalid 7-bit value')
        value |= (byte & 127) << (7 * step)
        if byte < 128:
            if step and value < (1 << (7 * step)):
                raise ValueError('overlong 7-bit value')
            return value, offset
    raise ValueError('unterminated 7-bit value')


def u32(data, offset):
    if offset < 0 or offset + 4 > len(data):
        raise ValueError('truncated 32-bit value')
    return struct.unpack_from('<I', data, offset)[0]


def inspect(data):
    if not 10 <= len(data) <= MAX_FILE_BYTES or data[:3] != b'XNB' or data[3] not in (ord('d'), ord('w')):
        raise ValueError('invalid XNB header')
    if data[4] != 5 or data[5] not in (0, 1) or u32(data, 6) != len(data):
        raise ValueError('unsupported XNB version, flags or declared length')
    offset = 10
    readers, offset = read_7bit(data, offset)
    if not 1 <= readers <= 128:
        raise ValueError('reader count outside subset')
    names = []
    for _ in range(readers):
        length, offset = read_7bit(data, offset)
        if not 1 <= length <= 4096 or offset + length + 4 > len(data):
            raise ValueError('reader record outside file')
        name = data[offset:offset + length].decode('utf-8')
        offset += length
        version = struct.unpack_from('<i', data, offset)[0]
        names.append((name, version))
        offset += 4
    shared, offset = read_7bit(data, offset)
    root, offset = read_7bit(data, offset)
    if shared != 0 or not 1 <= root <= readers:
        raise ValueError('shared resource or root outside subset')
    name, version = names[root - 1]
    if not known_texture_reader(name, version):
        raise ValueError('unsupported texture reader')
    texture_offset = offset
    if offset + 20 > len(data):
        raise ValueError('truncated texture header')
    format_id, width, height, levels, size = struct.unpack_from('<IIIII', data, offset)
    offset += 20
    if (format_id not in (4, 5, 6) or not 1 <= width <= 32768 or not 1 <= height <= 32768
            or width * height > MAX_PIXELS or levels != 1):
        raise ValueError('unsupported texture shape')
    block_bytes = 8 if format_id == 4 else 16
    expected = ((width + 3) // 4) * ((height + 3) // 4) * block_bytes
    if size != expected or offset + size != len(data):
        raise ValueError('mip byte count or trailing data mismatch')
    return {'reader_prefix_end': texture_offset, 'format': format_id,
            'width': width, 'height': height, 'payload_bytes': size,
            'flags': data[5], 'reader': name}


def decode_pixels(data, info):
    """Use Pillow's DDS decoder independently of the Rust image_dds codec."""
    from PIL import Image

    width, height = info['width'], info['height']
    payload = data[info['reader_prefix_end'] + 20:]
    fourcc = {4: b'DXT1', 5: b'DXT3', 6: b'DXT5'}[info['format']]
    header = (b'DDS ' + struct.pack('<6I', 124, 0x81007, height, width,
                                   len(payload), 0) + struct.pack('<I', 1)
              + bytes(44) + struct.pack('<II4s5I', 32, 4, fourcc, 0, 0, 0, 0, 0)
              + struct.pack('<5I', 0x1000, 0, 0, 0, 0))
    with Image.open(BytesIO(header + payload)) as image:
        return image.convert('RGBA')


def pixel_metrics(original, exported, before, after):
    from PIL import Image, ImageChops, ImageStat

    source = decode_pixels(original, before)
    candidate = decode_pixels(exported, after)
    reference = source.resize(candidate.size, Image.Resampling.LANCZOS)
    difference = ImageChops.difference(reference, candidate)
    rms = ImageStat.Stat(difference).rms
    mse = sum(value * value for value in rms[:3]) / 3
    def composited_psnr(background):
        canvas = Image.new('RGBA', candidate.size, (background,) * 3 + (255,))
        shown_reference = Image.alpha_composite(canvas, reference).convert('RGB')
        shown_candidate = Image.alpha_composite(canvas, candidate).convert('RGB')
        shown_rms = ImageStat.Stat(ImageChops.difference(shown_reference, shown_candidate)).rms
        shown_mse = sum(value * value for value in shown_rms) / 3
        return None if shown_mse == 0 else round(10 * math.log10(255 * 255 / shown_mse), 2)
    return {'rgb_psnr_db_vs_lanczos': None if mse == 0 else round(10 * math.log10(255 * 255 / mse), 2),
            'display_psnr_db_over_black': composited_psnr(0),
            'display_psnr_db_over_white': composited_psnr(255),
            'rgb_rms_vs_lanczos': [round(value, 2) for value in rms[:3]],
            'alpha_rms_vs_lanczos': round(rms[3], 2),
            'source_transparent_pixels': source.getchannel('A').histogram()[0],
            'candidate_transparent_pixels': candidate.getchannel('A').histogram()[0]}


def verify(source, candidate, pixels=False):
    original = read_snapshot(source)
    exported = read_snapshot(candidate)
    before, after = inspect(original), inspect(exported)
    prefix = before['reader_prefix_end']
    if (prefix != after['reader_prefix_end'] or original[:6] != exported[:6]
            or original[10:prefix] != exported[10:prefix]):
        raise ValueError('XNB target, flags, reader table or root identity changed')
    if (before['format'] != after['format'] or after['width'] > before['width']
            or after['height'] > before['height'] or len(exported) >= len(original)):
        raise ValueError('codec, dimensions or reduction gate mismatch')
    result = {'verification': 'detached_snapshot_structure_only',
            'runtime_compatibility': 'unverified', 'physical_savings_bytes': None,
            'source_sha256': hashlib.sha256(original).hexdigest(),
            'candidate_sha256': hashlib.sha256(exported).hexdigest(),
            'source_bytes': len(original), 'candidate_bytes': len(exported),
            'logical_savings_bytes': len(original) - len(exported),
            'source_shape': [before['width'], before['height']],
            'candidate_shape': [after['width'], after['height']],
            'format_id': before['format']}
    if pixels:
        result['pixel_metrics'] = pixel_metrics(original, exported, before, after)
        result['verification'] = 'detached_snapshot_structure_and_pixel_metrics'
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('candidate', type=Path)
    parser.add_argument('--pixels', action='store_true',
                        help='decode BC pixels independently with Pillow and compare to a Lanczos reference')
    args = parser.parse_args()
    print(json.dumps(verify(args.source, args.candidate, args.pixels), indent=2))


if __name__ == '__main__':
    main()
