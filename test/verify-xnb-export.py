#!/usr/bin/env python3
"""Independently check a detached, single-mip XNB Texture2D export."""

import argparse
import hashlib
from io import BytesIO
import json
import math
from pathlib import Path
import struct


def read_7bit(data, offset):
    value = 0
    for step in range(5):
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
    return struct.unpack_from('<I', data, offset)[0]


def inspect(data):
    if len(data) < 10 or data[:3] != b'XNB' or data[3] not in (ord('d'), ord('w')):
        raise ValueError('invalid XNB header')
    if data[4] != 5 or data[5] != 0 or u32(data, 6) != len(data):
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
        names.append(data[offset:offset + length].decode('utf-8'))
        offset += length
        if u32(data, offset) != 0:
            raise ValueError('reader version outside subset')
        offset += 4
    shared, offset = read_7bit(data, offset)
    root, offset = read_7bit(data, offset)
    if shared != 0 or not 1 <= root <= readers:
        raise ValueError('shared resource or root outside subset')
    name = names[root - 1]
    if name.split(',', 1)[0].strip() != 'Microsoft.Xna.Framework.Content.Texture2DReader':
        raise ValueError('unsupported texture reader')
    texture_offset = offset
    if offset + 20 > len(data):
        raise ValueError('truncated texture header')
    format_id, width, height, levels, size = struct.unpack_from('<IIIII', data, offset)
    offset += 20
    if format_id not in (4, 5, 6) or width == 0 or height == 0 or levels != 1:
        raise ValueError('unsupported texture shape')
    block_bytes = 8 if format_id == 4 else 16
    expected = ((width + 3) // 4) * ((height + 3) // 4) * block_bytes
    if size != expected or offset + size != len(data):
        raise ValueError('mip byte count or trailing data mismatch')
    return {'reader_prefix_end': texture_offset, 'format': format_id,
            'width': width, 'height': height, 'payload_bytes': size}


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
    original = source.read_bytes()
    exported = candidate.read_bytes()
    before, after = inspect(original), inspect(exported)
    prefix = before['reader_prefix_end']
    if (prefix != after['reader_prefix_end'] or original[:6] != exported[:6]
            or original[10:prefix] != exported[10:prefix]):
        raise ValueError('XNB target, flags, reader table or root identity changed')
    if (before['format'] != after['format'] or after['width'] > before['width']
            or after['height'] > before['height'] or len(exported) >= len(original)):
        raise ValueError('codec, dimensions or reduction gate mismatch')
    result = {'source_sha256': hashlib.sha256(original).hexdigest(),
            'candidate_sha256': hashlib.sha256(exported).hexdigest(),
            'source_bytes': len(original), 'candidate_bytes': len(exported),
            'logical_savings_bytes': len(original) - len(exported),
            'source_shape': [before['width'], before['height']],
            'candidate_shape': [after['width'], after['height']],
            'format_id': before['format']}
    if pixels:
        result['pixel_metrics'] = pixel_metrics(original, exported, before, after)
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
