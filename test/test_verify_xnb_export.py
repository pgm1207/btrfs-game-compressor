#!/usr/bin/env python3
"""Independent XNB verifier contracts; no pixel dependency for these fixtures."""
import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('verify_xnb_export', Path(__file__).with_name('verify-xnb-export.py'))
VERIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY)
READER = 'Microsoft.Xna.Framework.Content.Texture2DReader'


def seven_bit(value):
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return result


def texture(width=8, height=8, flags=0, name=READER, extra_reader=False):
    data = bytearray(b'XNBw\x05' + bytes([flags]) + bytes(4))
    readers = [('Game.UnusedReader', -1), (name, 0)] if extra_reader else [(name, 0)]
    data.extend(seven_bit(len(readers)))
    for reader_name, version in readers:
        encoded = reader_name.encode()
        data.extend(seven_bit(len(encoded)))
        data.extend(encoded)
        data.extend(struct.pack('<i', version))
    data.extend(b'\x00' + seven_bit(len(readers)))
    payload = bytes(((width + 3) // 4) * ((height + 3) // 4) * 8)
    data.extend(struct.pack('<5I', 4, width, height, 1, len(payload)))
    data.extend(payload)
    struct.pack_into('<I', data, 6, len(data))
    return bytes(data)


class VerifierTests(unittest.TestCase):
    def test_reader_allowlist_and_nonroot_versions(self):
        qualified = READER + ', FNA, Version=1.2.3.4, Culture=neutral, PublicKeyToken=null'
        self.assertEqual(VERIFY.inspect(texture(flags=1, name=qualified, extra_reader=True))['flags'], 1)
        for name in [READER + ', Custom.Framework', READER + ', FNA, Culture=en-US',
                     READER + ', FNA, Version=65536.0.0.0', READER + ', FNA, Version=１.0.0.0',
                     READER + ', FNA, PublicKeyToken=bad', READER + ', FNA, Culture=neutral, Culture=neutral',
                     READER + ', FNA, CodeBase=file:///tmp/code.dll', READER + '[Custom.Texture]']:
            with self.subTest(reader=name), self.assertRaises(ValueError):
                VERIFY.inspect(texture(name=name))

    def test_truncation_overlong_integers_mip_length_and_trailing_data(self):
        original = texture()
        for size in range(len(original)):
            with self.subTest(size=size), self.assertRaises(ValueError):
                VERIFY.inspect(original[:size])
        for encoded in [b'\x80\x00', b'\xff\xff\xff\xff\x08', b'\x80']:
            with self.assertRaises(ValueError):
                VERIFY.read_7bit(encoded, 0)
        header = VERIFY.inspect(original)['reader_prefix_end']
        bad_length = bytearray(original)
        struct.pack_into('<I', bad_length, header + 16, 31)
        trailing = bytearray(original + b'\x00')
        struct.pack_into('<I', trailing, 6, len(trailing))
        for data in [bad_length, trailing, texture(flags=2)]:
            with self.assertRaises(ValueError):
                VERIFY.inspect(data)

    def test_snapshot_and_retained_prefix_contract_without_media_decode(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, candidate = root / 'source.xnb', root / 'candidate.xnb'
            source.write_bytes(texture(16, 16, flags=1))
            candidate.write_bytes(texture(8, 8, flags=1))
            result = VERIFY.verify(source, candidate)
            self.assertEqual(result['logical_savings_bytes'], 96)
            self.assertIsNone(result['physical_savings_bytes'])
            self.assertEqual(result['runtime_compatibility'], 'unverified')
            candidate.write_bytes(texture(8, 8, flags=0))
            with self.assertRaises(ValueError):
                VERIFY.verify(source, candidate)
            linked = root / 'link.xnb'
            linked.symlink_to(source)
            with self.assertRaises(OSError):
                VERIFY.read_snapshot(linked)
            with self.assertRaises(ValueError):
                VERIFY.read_snapshot(root)


if __name__ == '__main__':
    unittest.main()
