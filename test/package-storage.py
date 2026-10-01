#!/usr/bin/env python3
"""Measure a lossless Hades PKG pass on disposable Btrfs copies, including backups.

Never modifies input packages. Retains all copies and backups for inspection.
Python is a developer-test dependency only, not part of the native runtime.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('packages', type=Path, help='source Hades package directory')
    parser.add_argument('--scratch', type=Path, required=True, help='new directory on Btrfs')
    parser.add_argument('--level', type=int, default=6)
    parser.add_argument('--backend', type=Path, default=Path(__file__).resolve().parents[1] / 'bgc-native')
    parser.add_argument('--sudo-stdin', action='store_true', help='read sudo credential once from stdin; never save it')
    parser.add_argument('--select-physical', action='store_true', help='selectively restore packages that fail per-file physical measurements')
    args = parser.parse_args()
    if not 1 <= args.level <= 15:
        parser.error('level must be 1..15')
    source = args.packages.resolve(strict=True)
    if args.scratch.resolve().is_relative_to(source):
        parser.error('scratch must be outside the source package directory')
    inputs = sorted(p for p in source.rglob('*.pkg') if p.is_file() and not p.is_symlink())
    if not inputs:
        parser.error('source contains no regular .pkg files')
    args.scratch.mkdir(parents=True, exist_ok=False)
    live = args.scratch / 'packages'
    live.mkdir()
    backend = str(args.backend.resolve())
    password = sys.stdin.readline() if args.sudo_stdin else None
    if password is None:
        subprocess.run(['sudo', '-v'], check=True)
    def native(*command, privileged=False):
        sudo = (['sudo', '-S', '-p', ''] if password is not None else ['sudo', '-n']) if privileged else []
        p = subprocess.run([*sudo, backend, *map(str, command)],
                           input=password if privileged else None,
                           text=True, capture_output=True, check=True)
        if p.stderr:
            print(p.stderr, file=sys.stderr, end='')
        return p.stdout.strip()
    def measure(path):
        disk, raw, referenced = map(int, native('measure-bytes', path, privileged=True).split('|'))
        return {'physical_bytes': disk, 'raw_extent_bytes': raw, 'referenced_bytes': referenced}
    for path in inputs:
        target = live / path.relative_to(source)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, target)
    native('compress', args.level, live, privileged=True)
    before = measure(live)
    result = {'input_directory': str(source), 'scratch': str(args.scratch.resolve()),
              'package_count': len(inputs), 'baseline': before,
              'apply': native('assets', 'apply', 'lossless', args.level, live)}
    after = measure(live)
    result['all_candidates_after'] = after
    if args.select_physical:
        rejected = []
        backup_path = live / '.bgc-assets-backup'
        for original in sorted(backup_path.rglob('*.pkg')):
            if not original.is_file() or original.is_symlink():
                continue
            relative = original.relative_to(backup_path)
            candidate = live / relative
            old = int(native('measure-file', original, privileged=True).split('|')[0])
            new = int(native('measure-file', candidate, privileged=True).split('|')[0])
            if new >= old:
                native('assets-restore-file', live, relative)
                rejected.append({'file': str(relative), 'original_physical': old, 'candidate_physical': new})
        result['rejected_packages'] = rejected
        after = measure(live)
    backup_path = live / '.bgc-assets-backup'
    backup = measure(backup_path) if backup_path.exists() else {'physical_bytes': 0}
    result.update(live_after=after, retained_backup=backup,
                  live_physical_reduction=before['physical_bytes']-after['physical_bytes'],
                  net_reduction_with_backup=before['physical_bytes']-after['physical_bytes']-backup['physical_bytes'])
    result['caveat'] = 'Independent-copy experiment only. Installed files unchanged. Filesystem metadata, snapshots and external sharing are not counted. Backups retained; no finalization performed.'
    (args.scratch / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
