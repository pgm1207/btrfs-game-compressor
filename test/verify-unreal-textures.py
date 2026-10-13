#!/usr/bin/env python3
"""Independently verify `unreal-texture-audit`/`unreal-texture-export` on real
cooked UE4 Texture2D `.uasset` payloads.

Usage: verify-unreal-textures.py BINARY FILE...

This re-implements the verified UE4 459-484 cooked `UTexture2D` layout (legacy
file version -6/-7, licensee 0, empty custom-version container) from the bytes
only: the cooked property tag stream, the format list with its declared skip
offsets, `FTexturePlatformData` and each mip's `FByteBulkData` header. It then
compares the audit records the binary prints, runs the detached exporter for
every fully inline texture outside the export subset, and checks the written DDS
header, mip payload and unchanged source. It decodes no pixels and proves no
runtime compatibility.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import struct
import subprocess
import sys
import tempfile

MAX_FILE_BYTES = 4 * 1024 * 1024 * 1024
BULK_PAYLOAD_AT_END = 0x1
BULK_COMPRESSED_ZLIB = 0x2
BULK_SINGLE_USE = 0x8
BULK_FORCE_INLINE = 0x40
BULK_SEPARATE_FILE = 0x100
BULK_OPTIONAL = 0x800
BULK_SIZE_64_BIT = 0x2000
BULK_KNOWN = (BULK_PAYLOAD_AT_END | BULK_COMPRESSED_ZLIB | BULK_SINGLE_USE
              | BULK_FORCE_INLINE | BULK_SEPARATE_FILE | BULK_OPTIONAL | BULK_SIZE_64_BIT)

# Byte layout `(block width, block height, bytes per block)` of the cooked pixel
# formats this verifier can size; a pixel format is a block of one.
LAYOUTS = {
    'PF_DXT1': (4, 4, 8), 'PF_BC4': (4, 4, 8),
    'PF_DXT3': (4, 4, 16), 'PF_DXT5': (4, 4, 16), 'PF_BC5': (4, 4, 16),
    'PF_BC6H': (4, 4, 16), 'PF_BC7': (4, 4, 16),
    'PF_B8G8R8A8': (1, 1, 4), 'PF_R8G8B8A8': (1, 1, 4), 'PF_G8': (1, 1, 1),
    'PF_G16': (1, 1, 2), 'PF_R16F': (1, 1, 2), 'PF_R32F': (1, 1, 4),
    'PF_A16B16G16R16': (1, 1, 8), 'PF_FloatRGBA': (1, 1, 8), 'PF_A32B32G32R32F': (1, 1, 16),
}
EXPORT_FORMATS = {
    'PF_DXT1': ('DXT1', None), 'PF_DXT3': ('DXT3', None), 'PF_DXT5': ('DXT5', None),
    'PF_BC7': ('DX10', 98), 'PF_BC4': ('DX10', 80), 'PF_BC5': ('DX10', 83),
    'PF_B8G8R8A8': ('DX10', 87), 'PF_R8G8B8A8': ('DX10', 28),
}


def read_snapshot(path):
    """Bounded read of one regular file; a directory or symlink is refused."""
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    before = os.fstat(fd)
    if not stat.S_ISREG(before.st_mode) or before.st_size > MAX_FILE_BYTES:
        os.close(fd)
        raise ValueError(f'{path}: not a bounded regular file')
    with os.fdopen(fd, 'rb', buffering=0) as source:
        data = source.read()
        after = os.fstat(source.fileno())
        if before.st_size != len(data) or (before.st_mtime_ns, before.st_ctime_ns) != (
                after.st_mtime_ns, after.st_ctime_ns):
            raise ValueError(f'{path}: changed during read')
    return data


def i32(data, offset):
    return struct.unpack_from('<i', data, offset)[0]


def u32(data, offset):
    return struct.unpack_from('<I', data, offset)[0]


def i64(data, offset):
    return struct.unpack_from('<q', data, offset)[0]


def fstring(data, offset):
    size = i32(data, offset)
    if size <= 0:
        raise ValueError(f'unexpected string at {offset}')
    end = offset + 4 + size
    return data[offset + 4:end - 1].decode('utf-8'), end


def names_and_exports(data):
    """Package summary prefix, name table and export table."""
    if u32(data, 0) != 0x9E2A83C1 or i32(data, 4) not in (-6, -7) or u32(data, 8) != 864:
        raise ValueError('not the verified Unreal package window')
    ue4 = u32(data, 12)
    if not 459 <= ue4 <= 484 or u32(data, 16) != 0 or u32(data, 20) != 0:
        raise ValueError('not the verified Unreal package window')
    header = i32(data, 24)
    _, offset = fstring(data, 28)
    name_count, name_offset = i32(data, offset + 4), i32(data, offset + 8)
    export_count, export_offset = i32(data, offset + 20), i32(data, offset + 24)
    import_count, import_offset = i32(data, offset + 28), i32(data, offset + 32)
    depends_field = offset + 36
    names = []
    cursor = name_offset
    for _ in range(name_count):
        name, cursor = fstring(data, cursor)
        names.append(name)
    imports = []
    for index in range(import_count):
        record = import_offset + index * 28
        imports.append(names[i32(data, record + 20)])
    exports = []
    for index in range(export_count):
        record = export_offset + index * 68
        class_index, _, _, object_name = (i32(data, record), i32(data, record + 4),
                                          i32(data, record + 8), i32(data, record + 12))
        size, position = i32(data, record + 24), i32(data, record + 28)
        class_name = (names[class_index - 1] if class_index > 0
                      else imports[-class_index - 1] if class_index < 0 else 'None')
        exports.append({'class_name': class_name, 'object': names[object_name],
                        'size': size, 'position': position})
    # Summary tail up to BulkDataStartOffset (same verified field chain).
    cursor = depends_field + 4
    cursor += 8  # StringAssetReferences count/offset
    cursor += 4  # ThumbnailTableOffset
    cursor += 16  # package GUID
    generations = i32(data, cursor)
    cursor += 4 + generations * 8
    for _ in range(2):  # saved-by and compatible FEngineVersion
        cursor += 10
        size = i32(data, cursor)
        cursor += 4 + (abs(size) * (1 if size > 0 else 2) if size else 0)
    cursor += 4  # CompressionFlags
    chunks = i32(data, cursor)
    cursor += 4 + chunks * 16
    cursor += 4  # PackageSource
    packages = i32(data, cursor)
    cursor += 4
    for _ in range(packages):
        size = i32(data, cursor)
        cursor += 4 + (abs(size) * (1 if size > 0 else 2) if size else 0)
    cursor += 4  # NumTextureAllocations
    cursor += 4  # AssetRegistryDataOffset
    if header > len(data) or (name_offset < 0) or (export_offset < 0):
        raise ValueError('summary extents outside the file')
    return names, exports, i64(data, cursor)


def expected_bytes(format_name, width, height):
    if format_name not in LAYOUTS:
        return None
    block_w, block_h, block_bytes = LAYOUTS[format_name]
    return ((width + block_w - 1) // block_w) * ((height + block_h - 1) // block_h) * block_bytes


def parse_texture(data, names, export, bulk_base):
    """The verified cooked Texture2D serialized layout, independently of Rust."""
    cursor = export['position']
    limit = cursor + export['size']

    def take(count):
        nonlocal cursor
        if cursor + count > limit:
            raise ValueError('texture record runs past its export extent')
        value = data[cursor:cursor + count]
        cursor += count
        return value

    def name():
        nonlocal cursor
        index, number = i32(data, cursor), i32(data, cursor + 4)
        cursor += 8
        if number != 0 or not 0 <= index < len(names):
            raise ValueError(f'bad FName at {cursor - 8}')
        return names[index]

    while True:
        tag = name()
        if tag == 'None':
            break
        kind = name()
        size = i32(data, cursor)
        cursor += 8  # size and ArrayIndex
        if kind == 'StructProperty':
            name()
            cursor += 16
        elif kind == 'BoolProperty':
            if size != 0 or data[cursor] > 1:
                raise ValueError('bad bool property')
            cursor += 1
            continue
        elif kind in ('ByteProperty', 'EnumProperty'):
            name()
        cursor += size
        if cursor > limit:
            raise ValueError('property value runs past its export extent')
    if i32(data, cursor) != 0:
        raise ValueError('unknown texture serialization prefix')
    cursor += 4
    strips = data[cursor:cursor + 4]
    cursor += 4
    if strips[0] & 1 == 0 or strips[2] & 1 == 0 or strips[1] or strips[3]:
        raise ValueError('texture strip flags outside the verified window')
    if i32(data, cursor) != 1:
        raise ValueError('texture is not cooked')
    cursor += 4
    entries = []
    while True:
        format_name = name()
        if format_name == 'None':
            break
        skip = i32(data, cursor)
        cursor += 4
        width, height, slices = i32(data, cursor), i32(data, cursor + 4), i32(data, cursor + 8)
        cursor += 12
        format_string, cursor = fstring(data, cursor)
        if format_string != format_name:
            raise ValueError('format name/string mismatch')
        first_mip, mip_count = i32(data, cursor), i32(data, cursor + 4)
        cursor += 8
        mips = []
        for index in range(mip_count):
            if i32(data, cursor) != 1:
                raise ValueError('mip is not cooked')
            cursor += 4
            flags = u32(data, cursor)
            element_count, size_on_disk = i32(data, cursor + 4), i32(data, cursor + 8)
            declared = i64(data, cursor + 12)
            cursor += 20
            if flags & ~BULK_KNOWN or flags & (BULK_COMPRESSED_ZLIB | BULK_SIZE_64_BIT
                                               | BULK_SEPARATE_FILE | BULK_OPTIONAL):
                raise ValueError(f'unsupported mip flags 0x{flags:x}')
            if element_count != size_on_disk or element_count < 0:
                raise ValueError('mip size fields disagree')
            if flags & BULK_PAYLOAD_AT_END:
                storage, start = 'bulk', bulk_base + declared
            else:
                if declared != cursor:
                    raise ValueError('inline mip offset does not point at its payload')
                storage, start = 'inline', cursor
                cursor += size_on_disk
            mip_width, mip_height = i32(data, cursor), i32(data, cursor + 4)
            cursor += 8
            expected = expected_bytes(format_name, mip_width, mip_height)
            if expected is not None and expected != size_on_disk:
                raise ValueError(f'mip {index} bytes disagree with the {format_name} shape')
            mips.append({'index': index, 'width': mip_width, 'height': mip_height,
                         'declared': size_on_disk, 'storage': storage, 'start': start,
                         'expected': expected})
        if cursor != skip:
            raise ValueError('platform data disagrees with its skip offset')
        entries.append({'format': format_string, 'width': width, 'height': height,
                        'slices': slices, 'first_mip': first_mip, 'mips': mips})
    if cursor != limit:
        raise ValueError('format list does not end at the export extent')
    if len(entries) != 1:
        raise ValueError('verifier knows single-entry textures only')
    return entries[0]


def expected_records(object_name, export_size, entry):
    storage = ('inline' if all(mip['storage'] == 'inline' for mip in entry['mips'])
               else 'bulk' if all(mip['storage'] == 'bulk' for mip in entry['mips']) else 'mixed')
    mip0 = entry['mips'][0]['declared']
    records = [f"UNREAL_TEXTURE|{object_name}|{entry['width']}|{entry['height']}|"
               f"{entry['format']}|{len(entry['mips'])}|{storage}|{mip0}"]
    declared = 0
    expected_total = 0
    for mip in entry['mips']:
        declared += mip['declared']
        if mip['expected'] is not None:
            expected_total += mip['expected']
        expected_word = mip['expected'] if mip['expected'] is not None else 'unknown'
        start = (mip['start'] if mip['storage'] == 'inline' or mip['start'] is not None
                 else 'unresolved')
        records.append(f"UNREAL_TEXTURE_MIP|{object_name}|0|{mip['index']}|{mip['width']}|"
                       f"{mip['height']}|{mip['declared']}|{mip['storage']}|{expected_word}|{start}")
    total_word = expected_total if entry['format'] in LAYOUTS else 'unknown'
    records.append(f"UNREAL_TEXTURE_TOTAL|{object_name}|{declared}|{total_word}|{export_size}")
    return records


def run(command):
    result = subprocess.run(command, capture_output=True, text=True)
    return result.returncode, result.stdout.splitlines()


def verify_texture(binary, path, workdir):
    data = read_snapshot(path)
    digest = hashlib.sha256(data).hexdigest()
    names, exports, bulk_base = names_and_exports(data)
    textures = [export for export in exports if export['class_name'] == 'Texture2D']
    if len(textures) != 1:
        raise ValueError(f'{path}: expected exactly one Texture2D export')
    export = textures[0]
    entry = parse_texture(data, names, export, bulk_base)
    expected = expected_records(export['object'], export['size'], entry)
    expected.append(f'UNREAL_TEXTURE_SUMMARY|1|1|1|0')

    code, lines = run([str(binary), 'unreal-texture-audit', str(path)])
    if code != 0 or lines != expected:
        raise ValueError(f'{path}: audit records differ\nwant {expected}\ngot  {lines}')
    if read_snapshot(path) != data:
        raise ValueError(f'{path}: source changed during audit')

    format_name = entry['format']
    inline = all(mip['storage'] == 'inline' for mip in entry['mips'])
    exportable = (format_name in EXPORT_FORMATS and inline and entry['slices'] == 1
                  and entry['first_mip'] == 0
                  and expected_bytes(format_name, entry['width'], entry['height']) is not None)
    output = workdir / (path.name + '.dds')
    code, lines = run([str(binary), 'unreal-texture-export', export['object'], str(path), str(output)])
    if not exportable:
        if code == 0 or output.exists():
            raise ValueError(f'{path}: export should have refused {format_name}')
        if len(lines) != 0:
            raise ValueError(f'{path}: refused export printed records: {lines}')
    else:
        if code != 0:
            raise ValueError(f'{path}: export failed: {lines}')
        dds = read_snapshot(output)
        fourcc = dds[84:88]
        wanted = EXPORT_FORMATS[format_name]
        if fourcc.decode('ascii') != wanted[0]:
            raise ValueError(f'{path}: DDS fourcc {fourcc!r} is not {wanted[0]}')
        if wanted[0] == 'DX10' and u32(dds, 128) != wanted[1]:
            raise ValueError(f'{path}: DDS DXGI format {u32(dds, 128)} is not {wanted[1]}')
        if (u32(dds, 16), u32(dds, 12), u32(dds, 28)) != (entry['width'], entry['height'], len(entry['mips'])):
            raise ValueError(f'{path}: DDS shape disagrees with the audit')
        payload = dds[148 if wanted[0] == 'DX10' else 128:]
        pieces = [data[mip['start']:mip['start'] + mip['declared']] for mip in entry['mips']]
        if payload != b''.join(pieces):
            raise ValueError(f'{path}: DDS payload is not the inline mip bytes')
        if lines != [f"UNREAL_TEXTURE_EXPORT|{export['object']}|{entry['width']}|{entry['height']}|"
                     f"{format_name}|{len(entry['mips'])}|{sum(m['declared'] for m in entry['mips'])}|"
                     f"{len(dds)}|EXPORTED"]:
            raise ValueError(f'{path}: export record mismatch: {lines}')
        if read_snapshot(path) != data:
            raise ValueError(f'{path}: source changed during export')
    return {'file': str(path), 'sha256': digest, 'object': export['object'],
            'shape': [entry['width'], entry['height']], 'format': format_name,
            'mips': len(entry['mips']), 'exported': exportable}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path, help='bgc-native built with the development-audits feature')
    parser.add_argument('files', type=Path, nargs='+')
    args = parser.parse_args()
    if not args.binary.is_file():
        raise SystemExit(f'{args.binary}: binary not found')
    results = []
    with tempfile.TemporaryDirectory(prefix='bgc-unreal-verify-') as temp:
        for path in args.files:
            results.append(verify_texture(args.binary, path, Path(temp)))
    print(json.dumps({'verification': 'independent_layout_and_payload_check',
                      'runtime_compatibility': 'unverified', 'files': results}, indent=2))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError) as error:
        print(f'verification failed: {error}', file=sys.stderr)
        sys.exit(1)
