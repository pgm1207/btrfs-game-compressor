#!/usr/bin/env python3
"""Checkpointed recovery of lossless operations after a lost asset-run journal.

Never reapplies lossy assets: their previous completion state is unknown.
Logs and accounting metadata contain no asset backup copies.
"""
import argparse
import collections
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


def atomic_json(path, value):
    """Durably replace *path* without using a predictable temporary name."""
    descriptor, temporary_name = tempfile.mkstemp(
        dir=path.parent, prefix=f'.{path.name}.', suffix='.tmp')
    temporary = Path(temporary_name)
    published = False
    try:
        with os.fdopen(descriptor, 'w') as output:
            json.dump(value, output, indent=2)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        published = True
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if not published:
            try:
                temporary.unlink()
            except FileNotFoundError:
                pass


def running(root):
    needle = str(root) + '/'
    for process in Path('/proc').iterdir():
        if not process.name.isdigit():
            continue
        try:
            maps = (process / 'maps').read_text()
            maps = maps.replace(r'\040', ' ').replace(r'\134', '\\')
            if needle in maps:
                return True
        except (PermissionError, FileNotFoundError, ProcessLookupError):
            continue
    return False


def footprint(root):
    logical = allocated = files = 0
    device = root.stat().st_dev
    for parent, directories, names in os.walk(root, followlinks=False):
        directories[:] = [name for name in directories
                          if name != '.bgc-assets-backup'
                          and not (Path(parent) / name).is_symlink()
                          and (Path(parent) / name).stat().st_dev == device]
        for name in names:
            path = Path(parent) / name
            metadata = path.lstat()
            if path.is_symlink() or not path.is_file() or metadata.st_dev != device:
                continue
            logical += metadata.st_size
            allocated += metadata.st_blocks * 512
            files += 1
    return dict(logical_bytes=logical, allocated_reference_bytes=allocated, files=files)


def chart(state, destination):
    counts = collections.Counter(row['status'] for row in state['games'])
    lines = ['# Steam compaction recovery pass', '',
             'Previous lossy-run journal was lost. Previous savings cannot be reconstructed.',
             ('Supported Balanced assets, Zstd and dedupe are enabled; unsupported assets remain unchanged.'
              if state.get('apply_assets') else
              'This pass runs lossless Zstd and dedupe only; it does not reapply 1080p assets.'), '',
             '| Result | Games |', '|---|---:|']
    lines += [f'| {status} | {count} |' for status, count in sorted(counts.items())]
    lines += ['', '| Game | Logical size (GiB) | Allocated references before → after (GiB) | Result |',
              '|---|---:|---:|---|']
    for row in state['games']:
        before = row.get('before')
        after = row.get('after')
        logical = f"{before['logical_bytes'] / 2**30:.3f}" if before else '—'
        change = (f"{before['allocated_reference_bytes'] / 2**30:.3f} → "
                  f"{after['allocated_reference_bytes'] / 2**30:.3f}") if before and after else '—'
        lines.append(f"| {row['name'].replace('|', '/')} | {logical} | {change} | {row['status']} |")
    lines += ['', 'Allocated references can count shared blocks multiple times; they are not net space freed.',
              'Exact extent savings require privileged measurement. Operation success is not runtime validation.',
              'Existing recovery data is preserved. No new asset backups are created.']
    if state.get('apply_assets'):
        measured = [row for row in state['games'] if 'after_assets' in row]
        saved = sum(row['before']['logical_bytes'] - row['after_assets']['logical_bytes'] for row in measured)
        lines += ['', f'Observed logical asset reduction: {saved / 2**30:.3f} GiB across {len(measured)} measured installs.',
                  'Includes partial operations if a stage failed; not physical savings or full-format coverage.']
    destination.write_text('\n'.join(lines) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument('--state-dir', type=Path, required=True)
    parser.add_argument('--apply-assets-no-backup', action='store_true',
                        help='Irreversibly apply supported Balanced assets before compression')
    args = parser.parse_args()
    args.state_dir.mkdir(parents=True, exist_ok=True)
    # Lock survives restarts as an inode but releases when the process exits.
    import fcntl
    with (args.state_dir / 'run.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return run_job(args)


def run_job(args):
    journal = args.state_dir / 'results.json'
    backend = args.state_dir / 'bgc-native-fixed'
    logs = args.state_dir / 'logs'
    logs.mkdir(exist_ok=True)
    if journal.exists():
        state = json.loads(journal.read_text())
        if state.get('apply_assets', False) != args.apply_assets_no_backup:
            raise RuntimeError('Journal mode differs; use a separate state directory')
        if not backend.is_file():
            raise RuntimeError('Checkpoint backend missing; refusing to resume with a different build')
        for process in Path('/proc').iterdir():
            if not process.name.isdigit():
                continue
            try:
                if (process / 'exe').resolve() == backend.resolve():
                    raise RuntimeError('A previous backend child is still running; refusing overlapping recovery')
            except (PermissionError, FileNotFoundError, ProcessLookupError):
                continue
    else:
        status = subprocess.check_output([str(args.repo / 'btrfs-game-compressor'), '--status', '--json'], text=True)
        discovery = json.loads(status)
        shutil.copy2(args.repo / 'bgc-native', backend)
        state = {'started': time.time(), 'mode': 'lossless-recovery-no-lossy-reapplication',
                 'apply_assets': args.apply_assets_no_backup,
                 'previous_asset_accounting': 'unavailable after server restart',
                 'games': [dict(name=g['name'], path=g['path'], status='pending') for g in discovery['games']]}
        atomic_json(journal, state)

    def save():
        atomic_json(journal, state)
        chart(state, args.state_dir / 'stats.md')

    def stage(row, index, name, arguments):
        if row.get(name, {}).get('rc') == 0:
            return 0
        row['status'] = name + '_in_progress'
        save()
        log = logs / f'{index:03d}-{name}.log'
        environment = dict(os.environ)
        environment.pop('BGC_VERBOSE', None)
        with log.open('a') as output:
            output.write(f'\nAttempt at {time.time()}\n')
            output.flush()
            child = subprocess.run([str(backend), *map(str, arguments)], stdout=output,
                                   stderr=subprocess.STDOUT, env=environment)
        row[name] = {'rc': child.returncode, 'log': str(log), 'finished': time.time()}
        save()
        return child.returncode

    for index, row in enumerate(state['games'], 1):
        if row['status'] == 'lossless_operations_completed_assets_unverified':
            continue
        root = Path(row['path'])
        try:
            if root.is_symlink() or not root.is_dir():
                row['status'] = 'skipped_invalid_root'
            elif running(root):
                row['status'] = 'skipped_running'
            elif (root / '.bgc-assets-backup').exists():
                row['status'] = 'skipped_existing_recovery_data'
            else:
                if 'before' not in row:
                    row['before'] = footprint(root)
                    save()
                if args.apply_assets_no_backup:
                    if row.get('assets', {}).get('rc') != 0:
                        if row.get('assets_started'):
                            row['status'] = 'asset_state_uncertain_manual_recovery_required'
                            save()
                            continue
                        row['assets_started'] = time.time()
                        save()
                        rc = stage(row, index, 'assets', ['assets', 'apply-no-backup', 'balanced', 1, root])
                        row['after_assets'] = footprint(root)
                        save()
                        if rc:
                            row['status'] = 'assets_failed_may_be_partial'
                            save()
                            continue
                    if running(root):
                        row['status'] = 'stopped_game_started'
                        save()
                        continue
                if stage(row, index, 'compression', ['compress', 1, root]):
                    row['status'] = 'compression_failed'
                elif running(root):
                    row['status'] = 'stopped_game_started'
                else:
                    # Use one named private scratch directory per install so a
                    # cancelled dedupe can reuse it without leaking many trees.
                    scratch = args.state_dir / f'dedupe-scratch-{index:03d}'
                    scratch.mkdir(exist_ok=True)
                    rc = stage(row, index, 'dedupe', ['dedupe', root, scratch])
                    row['after'] = footprint(root)
                    row['status'] = 'dedupe_failed' if rc else 'lossless_operations_completed_assets_unverified'
        except Exception as error:
            row['status'] = 'runner_error'
            row['error'] = str(error)
        save()
        print(f"[{index}/{len(state['games'])}] {row['name']}: {row['status']}", flush=True)
    state['finished'] = time.time()
    save()
    print('STATS', args.state_dir / 'stats.md', flush=True)
    return 0 if all(row['status'] == 'lossless_operations_completed_assets_unverified'
                    for row in state['games']) else 1


if __name__ == '__main__':
    raise SystemExit(main())
