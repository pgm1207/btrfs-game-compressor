#!/usr/bin/env python3
"""Read-only independent Godot 3 PCM/IMA PCK check (no Rust parser).

Usage: verify-godot3-samples.py SOURCE CANDIDATE
Decoder follows scene/resources/audio_stream_sample.cpp in Godot 3.x:
zero initial state, no headers, interleaved channel bytes, low nibble first.
"""
import hashlib
import math
import struct
import sys

STEP = (7,8,9,10,11,12,13,14,16,17,19,21,23,25,28,31,34,37,41,45,
        50,55,60,66,73,80,88,97,107,118,130,143,157,173,190,209,230,
        253,279,307,337,371,408,449,494,544,598,658,724,796,876,963,
        1060,1166,1282,1411,1552,1707,1878,2066,2272,2499,2749,3024,
        3327,3660,4026,4428,4871,5358,5894,6484,7132,7845,8630,9493,
        10442,11487,12635,13899,15289,16818,18500,20350,22385,24623,
        27086,29794,32767)
INDEX = (-1,-1,-1,-1,2,4,6,8) * 2


class Reader:
    def __init__(self, data):
        self.data, self.pos = data, 0

    def take(self, count):
        assert 0 <= count <= len(self.data) - self.pos, 'truncated input'
        value = self.data[self.pos:self.pos + count]
        self.pos += count
        return value

    def number(self, fmt):
        return struct.unpack('<' + fmt, self.take(struct.calcsize('<' + fmt)))[0]

    def string(self, count=None):
        if count is None:
            count = self.number('I')
        return self.take(count).rstrip(b'\0').decode('utf-8')


def pack(path):
    with open(path, 'rb') as stream:
        r = Reader(stream.read())
    assert r.take(4) == b'GDPC' and r.number('I') == 1
    r.pos = 84
    entries = {}
    for _ in range(r.number('I')):
        name = r.string()
        offset, size = r.number('Q'), r.number('Q')
        digest = r.take(16)
        assert size <= len(r.data) - offset
        data = r.data[offset:offset + size]
        assert hashlib.md5(data).digest() == digest, name
        assert name not in entries
        entries[name] = data
    return entries


def resource(data):
    r = Reader(data)
    assert r.take(4) == b'RSRC'
    assert r.number('I') == 0 and r.number('I') == 0
    assert r.number('I') == 3
    r.number('I')
    assert r.number('I') == 3
    cls = r.string()
    assert r.number('Q') == 0
    r.take(56)
    strings = [r.string() for _ in range(r.number('I'))]
    assert r.number('I') == 0
    assert r.number('I') == 1
    r.string()
    r.pos = r.number('Q')
    assert r.string() == cls
    props = {}
    for _ in range(r.number('I')):
        index = r.number('I')
        name = r.string(index & 0x7fffffff) if index & 0x80000000 else strings[index]
        kind = r.number('I')
        if kind == 31:
            value = r.take(r.number('I'))
        elif kind == 5:
            value = r.string()
        elif kind == 1:
            value = None
        else:
            value = r.number({2: 'I', 3: 'i', 4: 'f', 40: 'q', 41: 'd'}[kind])
        assert name not in props
        props[name] = value
    return cls, props


def decode(data, frames, channels, channel):
    predictor = index = 0
    out = []
    for frame in range(frames):
        byte = data[(frame // 2) * channels + channel]
        nibble = byte >> 4 if frame % 2 else byte & 15
        step = STEP[index]
        # Godot takes the old step, then updates/clamps the index.
        index = max(0, min(88, index + INDEX[nibble]))
        delta = step // 8
        for mask, divisor in ((1, 4), (2, 2), (4, 1)):
            if nibble & mask:
                delta += step // divisor
        predictor = max(-32768, min(32767, predictor + (-delta if nibble & 8 else delta)))
        out.append(predictor)
    return out


def main():
    source, candidate = map(pack, sys.argv[1:])
    assert source.keys() == candidate.keys()
    checked, worst, saved = 0, math.inf, 0
    for name, old in source.items():
        new = candidate[name]
        if old == new:
            continue
        assert name.endswith('.sample'), 'unexpected changed entry: ' + name
        old_cls, a = resource(old)
        new_cls, b = resource(new)
        assert old_cls == new_cls == 'AudioStreamSample'
        assert a.get('format', 0) == 1 and b.get('format', 0) == 2
        pcm, ima = a.pop('data'), b.pop('data')
        a.pop('format'); b.pop('format')
        assert a == b, 'changed sample metadata: ' + name
        channels = 2 if a.get('stereo', 0) else 1
        assert len(pcm) % (2 * channels) == 0
        frames = len(pcm) // (2 * channels)
        assert len(ima) * 2 == frames * channels, 'changed frame count: ' + name
        samples = struct.unpack('<' + str(len(pcm) // 2) + 'h', pcm)
        for channel in range(channels):
            original = samples[channel::channels]
            decoded = decode(ima, frames, channels, channel)
            signal = sum(x*x for x in original)
            noise = sum((x-y)**2 for x, y in zip(original, decoded))
            snr = 10 * math.log10(signal / noise) if noise else math.inf
            assert snr >= 20, (name, channel, snr)
            worst = min(worst, snr)
        checked += 1
        saved += len(old) - len(new)
    assert checked > 0
    print(f'PASS: {checked} changed samples; per-channel minimum SNR {worst:.2f} dB; '
          f'{saved:,} resource bytes saved. All entry MD5s, frame counts and metadata verified.')


if __name__ == '__main__':
    main()
