#!/usr/bin/env python3
"""Benchmark Btrfs levels on disposable, independently allocated file copies.

Developer diagnostic only; Python is not a runtime dependency of the tool.
Reads/hashes installed files but never rewrites them or drops system caches.
Run on representative inputs, not as a whole-game savings projection.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        while chunk := stream.read(1024 * 1024):
            h.update(chunk)
    return h.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--backend', type=Path, default=Path(__file__).resolve().parents[1] / 'bgc-native')
    parser.add_argument('--scratch', type=Path, required=True, help='new directory on Btrfs; retained for inspection')
    parser.add_argument('--levels', type=int, nargs='+', default=[3, 6, 9])
    parser.add_argument('--reads', type=int, default=5)
    parser.add_argument('--sudo-stdin', action='store_true', help='read sudo credential once from stdin; never save it')
    parser.add_argument('files', type=Path, nargs='+')
    args = parser.parse_args()
    if args.reads < 1 or args.reads > 20 or any(level < 1 or level > 15 for level in args.levels):
        parser.error('reads must be 1..20 and levels 1..15')
    files = []
    for path in args.files:
        if path.is_symlink() or not path.is_file():
            parser.error(f'not a regular, non-symlink input: {path}')
        files.append(path.resolve())
    args.scratch.mkdir(parents=True, exist_ok=False)
    backend = str(args.backend.resolve())
    # Authenticate interactively outside the tool. Never save a password in a
    # report, file, environment variable or command-line argument.
    password = sys.stdin.readline() if args.sudo_stdin else None
    if password is None:
        subprocess.run(['sudo', '-v'], check=True)
    def native(*command):
        sudo = ['sudo', '-S', '-p', ''] if password is not None else ['sudo', '-n']
        p = subprocess.run([*sudo, backend, *map(str, command)], input=password,
                           check=True, text=True, capture_output=True)
        return p.stdout.strip()
    expected = [digest(path) for path in files]
    result = {'inputs': [{'path': str(p), 'bytes': p.stat().st_size, 'sha256': h}
                         for p, h in zip(files, expected)], 'levels': [],
              'caveat': 'Warm read+SHA256 timings, not isolated decoder CPU time or in-game performance. No cache flushing. Sample-only savings.'}
    for level in args.levels:
        directory = args.scratch / f'zstd-{level}'
        directory.mkdir()
        copies = []
        for i, path in enumerate(files):
            target = directory / f'{i:03d}-{path.name}'
            shutil.copyfile(path, target)  # no reflinks: independent extents
            copies.append(target)
        started = time.monotonic()
        native('compress', level, directory)
        encoding_seconds = time.monotonic() - started
        physical, raw, referenced = map(int, native('measure-bytes', directory).split('|'))
        for copy, checksum in zip(copies, expected):
            if digest(copy) != checksum:
                raise RuntimeError(f'content verification failed: {copy}')
        reads = []
        for _ in range(args.reads):
            started = time.monotonic()
            for copy, checksum in zip(copies, expected):
                if digest(copy) != checksum:
                    raise RuntimeError(f'content verification failed: {copy}')
            reads.append(time.monotonic() - started)
        row = {'level': level, 'physical_bytes': physical, 'raw_extent_bytes': raw,
               'referenced_bytes': referenced, 'encoding_seconds': encoding_seconds,
               'warm_read_hash_seconds': reads}
        result['levels'].append(row)
        (args.scratch / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(row), flush=True)
    print(f'Report: {args.scratch / "result.json"}')


if __name__ == '__main__':
    main()
