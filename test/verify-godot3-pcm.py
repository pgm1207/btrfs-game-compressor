#!/usr/bin/env python3
"""Independent, read-only check of an Ultra Performance PCM-only candidate.

Does not prove Godot loading or perceptual quality. SOURCE must already contain
the same music as CANDIDATE so only .sample entries may differ.
"""
import collections
from pathlib import Path
import runpy
import sys

helpers = runpy.run_path(str(Path(__file__).with_name('verify-godot3-samples.py')))
pack, resource = helpers['pack'], helpers['resource']


def main():
    source, candidate = map(pack, sys.argv[1:3])
    target_rate = int(sys.argv[3]) if len(sys.argv) > 3 else 22050
    assert source.keys() == candidate.keys()
    checked = saved = 0
    rates = collections.Counter()
    max_duration_error = 0.0
    for name, old in source.items():
        new = candidate[name]
        if old == new:
            continue
        assert name.endswith('.sample'), 'unexpected changed entry: ' + name
        cls_a, a = resource(old)
        cls_b, b = resource(new)
        assert cls_a == cls_b == 'AudioStreamSample'
        assert a.get('format', 0) in (0, 1) and b.get('format', 0) == 0
        rate_a, rate_b = a.get('mix_rate', 44100), b.get('mix_rate', 44100)
        assert rate_b == min(rate_a, target_rate)
        assert a.get('stereo', 0) == b.get('stereo', 0)
        channels = 2 if a.get('stereo', 0) else 1
        pcm_a, pcm_b = a.pop('data'), b.pop('data')
        stride_a = channels * (2 if a.get('format', 0) == 1 else 1)
        assert len(pcm_a) % stride_a == 0 and len(pcm_b) % 4 == 0
        frames_a, frames_b = len(pcm_a) // stride_a, len(pcm_b) // channels
        alignment = 4 // channels
        expected_frames = (frames_a * rate_b + rate_a // 2) // rate_a
        expected_frames -= expected_frames % alignment
        assert frames_b == expected_frames
        duration_error = abs(frames_a / rate_a - frames_b / rate_b)
        assert duration_error <= (alignment + 0.5) / rate_b
        max_duration_error = max(max_duration_error, duration_error)
        for key in ('loop_begin', 'loop_end'):
            if key in a:
                expected = min((a[key] * rate_b + rate_a // 2) // rate_a, frames_b)
                assert b[key] == expected
                a.pop(key); b.pop(key)
        for key in ('format', 'mix_rate'):
            a.pop(key, None); b.pop(key, None)
        assert a == b, 'unexpected metadata change: ' + name
        checked += 1
        saved += len(old) - len(new)
        rates[rate_b] += 1
    assert checked > 0
    print(f'PASS: {checked} PCM8 samples; rates={dict(rates)}; '
          f'maximum duration rounding {max_duration_error * 1000:.3f} ms; '
          f'{saved:,} resource bytes saved. All entry MD5s and unchanged assets verified.')


if __name__ == '__main__':
    main()
