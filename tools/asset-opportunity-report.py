#!/usr/bin/env python3
"""Read-only, fixed-backend asset candidate census from a saved --status --json."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def fixed_backend(source, output_dir):
    source_hash = sha256(source)
    destination = output_dir / f'bgc-native-{source_hash[:16]}'
    if destination.exists():
        if not destination.is_file() or sha256(destination) != source_hash:
            raise ValueError(f'backend snapshot has unexpected contents: {destination}')
        return destination, source_hash
    with tempfile.NamedTemporaryFile(prefix='.bgc-native-', dir=output_dir, delete=False) as temporary:
        temporary_path = Path(temporary.name)
        try:
            with source.open('rb') as original:
                shutil.copyfileobj(original, temporary)
            temporary.flush()
            os.fsync(temporary.fileno())
            if sha256(temporary_path) != source_hash:
                raise ValueError('backend changed while being copied; retry with a stable build')
            temporary_path.chmod(0o700)
            temporary_path.replace(destination)
        finally:
            temporary_path.unlink(missing_ok=True)
    return destination, source_hash


def parse_plan(stdout, profile):
    rows = [line for line in stdout.splitlines() if line.startswith('ASSETS|')]
    if len(rows) != 1:
        raise ValueError(f'expected one ASSETS response, got {len(rows)}')
    fields = rows[0].split('|')
    labels = {
        'balanced': 'Balanced (1080p)',
        'performance': 'Performance (720p)',
        'ultra-performance': 'Ultra Performance (480p; WAV up to 11.025 kHz / 8-bit)',
        'quality': 'Quality (1440p)',
        'ultra-quality': 'Ultra Quality (4K)',
    }
    if len(fields) != 17 or fields[:3] != ['ASSETS', 'plan', labels[profile]]:
        raise ValueError('unexpected planner response schema or profile')
    if any(not value.isascii() or not value.isdecimal() for value in fields[3:]):
        raise ValueError('planner response contains a non-integer count')
    numbers = list(map(int, fields[3:]))
    count, before, after = numbers[:3]
    if after > before or (count == 0 and before != after):
        raise ValueError('planner response has inconsistent candidate totals')
    return {
        'estimate_scope': 'asset-plan loose/standalone reductions plus separate Godot 4 PCK texture audits',
        'candidate_files': count,
        'candidate_source_bytes': before,
        'candidate_output_bytes': after,
        'planner_logical_savings_bytes': before - after,
        'estimated_logical_savings_bytes': before - after,
        'inventory_packed_bytes': numbers[11],
        'inventory_audio_bytes': numbers[12],
        'physical_savings_bytes': None,
        'raw_response': rows[0],
    }


def raise_walk_error(error):
    raise error


def godot4_pcks(root):
    """Find ordinary same-device PCK files without following links or mounts."""
    device = root.stat().st_dev
    for parent, directories, names in os.walk(root, followlinks=False, onerror=raise_walk_error):
        directories[:] = [name for name in directories if name != '.bgc-assets-backup'
                          and not (Path(parent) / name).is_symlink()
                          and (Path(parent) / name).stat().st_dev == device]
        for name in names:
            if not name.lower().endswith('.pck'):
                continue
            path = Path(parent) / name
            metadata = path.lstat()
            if not path.is_file() or path.is_symlink() or metadata.st_dev != device:
                continue
            with path.open('rb') as source:
                header = source.read(8)
            if len(header) == 8 and header[:4] == b'GDPC' and int.from_bytes(header[4:], 'little') in (3, 4):
                yield path, metadata.st_size


def plain_xnbs(root, deadline=None):
    """Find uncompressed desktop XNB v5 files for optional development audits."""
    def check_deadline():
        if deadline is not None and time.monotonic() >= deadline:
            raise subprocess.TimeoutExpired('xnb discovery', 0)

    check_deadline()
    device = root.stat().st_dev
    for parent, directories, names in os.walk(root, followlinks=False, onerror=raise_walk_error):
        check_deadline()
        retained = []
        for name in directories:
            check_deadline()
            if name == '.bgc-assets-backup':
                continue
            child = (Path(parent) / name).lstat()
            if stat.S_ISDIR(child.st_mode) and child.st_dev == device:
                retained.append(name)
        directories[:] = retained
        for name in names:
            check_deadline()
            if not name.lower().endswith('.xnb'):
                continue
            path = Path(parent) / name
            metadata = path.lstat()
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_dev != device or metadata.st_size > 64 * 1024 * 1024:
                continue
            # Refuse a final symlink/FIFO swapped in between discovery and open.
            # Ancestor containment and filesystem snapshot consistency are NOT
            # guaranteed by this read-only research scanner.
            fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
            with os.fdopen(fd, 'rb', buffering=0) as source:
                opened = os.fstat(source.fileno())
                if (not stat.S_ISREG(opened.st_mode) or opened.st_dev != metadata.st_dev
                        or opened.st_ino != metadata.st_ino or opened.st_size != metadata.st_size
                        or opened.st_mtime_ns != metadata.st_mtime_ns or opened.st_ctime_ns != metadata.st_ctime_ns):
                    raise ValueError(f'XNB source changed during discovery: {path}')
                header = source.read(10)
                after = os.fstat(source.fileno())
                if (after.st_size, after.st_mtime_ns, after.st_ctime_ns) != (
                        opened.st_size, opened.st_mtime_ns, opened.st_ctime_ns):
                    raise ValueError(f'XNB source changed during header reading: {path}')
            if (len(header) == 10 and header[:3] == b'XNB' and header[3] in (ord('d'), ord('w'))
                    and header[4] == 5 and header[5] in (0, 1)
                    and int.from_bytes(header[6:], 'little') == metadata.st_size):
                yield path, metadata.st_size


def parse_xnb_audit(stdout, source_bytes, max_edge):
    def record(tag, fields):
        rows = [line.split('|') for line in stdout.splitlines() if line.startswith(tag + '|')]
        if len(rows) != 1 or len(rows[0]) != fields + 1:
            raise ValueError(f'unexpected {tag} schema')
        return rows[0][1:]

    def numbers(values):
        if any(not value.isascii() or not value.isdecimal() for value in values):
            raise ValueError('unexpected XNB numeric schema')
        return list(map(int, values))

    if not 64 <= max_edge <= 8192:
        raise ValueError('XNB target edge outside export subset')
    texture = [line.split('|') for line in stdout.splitlines() if line.startswith('XNB_TEXTURE|')]
    status = [line.split('|') for line in stdout.splitlines() if line.startswith('XNB_TEXTURE_STATUS|')]
    if len(status) != 1 or len(status[0]) != 3:
        raise ValueError('unexpected XNB audit status schema')
    if status[0][1] == 'OPAQUE' and status[0][2] in {
            'UNSUPPORTED_VERSION', 'COMPRESSED_PAYLOAD_NOT_DECODED', 'UNSUPPORTED_TARGET',
            'SHARED_RESOURCES_NOT_PARSED', 'NULL_ROOT', 'CUSTOM_OR_UNSUPPORTED_ROOT_READER',
            'UNSUPPORTED_SURFACE_FORMAT'}:
        return None
    if status[0][1:] != ['METADATA_ONLY', 'METADATA_ONLY']:
        raise ValueError('unexpected XNB audit status value')
    header = record('XNB_CONTENT_HEADER', 4)
    version, flags, declared_size = numbers(header[1:])
    if (header[0] not in ('w', 'd') or version != 5 or flags not in (0, 1)
            or declared_size != source_bytes or source_bytes > 64 * 1024 * 1024):
        raise ValueError('XNB audit header or source length mismatch')
    shared = numbers(record('XNB_SHARED', 1))[0]
    root = numbers(record('XNB_ROOT', 1))[0]
    if shared != 0 or not 1 <= root <= 128:
        raise ValueError('XNB shared resources or root outside subset')
    if len(texture) != 1 or len(texture[0]) != 5:
        raise ValueError('unexpected XNB texture schema')
    surface, width, height, levels = numbers(texture[0][1:])
    if not 1 <= width <= 32768 or not 1 <= height <= 32768:
        raise ValueError('XNB dimensions outside audit subset')
    if (surface not in (4, 5, 6) or levels != 1 or width * height > 16_777_216
            or max(width, height) <= 512 or min(width, height) <= 64):
        return None
    edge = max(max_edge, (max(width, height) + 1) // 2)
    new_width = min(width, edge) if width >= height else (width * min(height, edge) + height // 2) // height
    new_height = min(height, edge) if height > width else (height * min(width, edge) + width // 2) // width
    if (new_width, new_height) == (width, height) or min(new_width, new_height) < 64:
        return None
    block = 8 if surface == 4 else 16
    old_payload = ((width + 3) // 4) * ((height + 3) // 4) * block
    new_payload = ((new_width + 3) // 4) * ((new_height + 3) // 4) * block
    level, mip_width, mip_height, offset, size = numbers(record('XNB_MIP', 5))
    if (level != 0 or (mip_width, mip_height) != (width, height)
            or size != old_payload or offset < 30 or offset + size != source_bytes
            or old_payload <= new_payload):
        raise ValueError('XNB payload or candidate length is inconsistent')
    return {'format_id': surface, 'width': width, 'height': height,
            'candidate_width': new_width, 'candidate_height': new_height,
            'theoretical_logical_savings_bytes': old_payload - new_payload}


def xnb_opportunities(backend, root, max_edge, deadline):
    """Keep optional research failures independent from production estimates."""
    result = {'xnb_candidates': [], 'xnb_theoretical_logical_savings_bytes': 0,
              'xnb_audit_status': 'ok', 'xnb_header_files': 0, 'xnb_audited_files': 0,
              'xnb_name_skips': 0, 'xnb_error_count': 0, 'xnb_error_samples': []}

    def error(message):
        result['xnb_error_count'] += 1
        result['xnb_audit_status'] = 'partial error'
        if len(result['xnb_error_samples']) < 16:
            result['xnb_error_samples'].append(message[-1000:])

    try:
        for xnb, source_bytes in plain_xnbs(root, deadline):
            result['xnb_header_files'] += 1
            if atlas_hint(xnb):
                result['xnb_name_skips'] += 1
                continue
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise subprocess.TimeoutExpired('xnb-texture-audit', 0)
            try:
                audited = subprocess.run([str(backend), 'xnb-texture-audit', str(xnb)],
                                         capture_output=True, text=True, timeout=remaining, check=False)
                if audited.returncode:
                    raise ValueError(f'XNB audit exited {audited.returncode} for {xnb}: {audited.stderr[-1000:]}')
                details = parse_xnb_audit(audited.stdout, source_bytes, max_edge)
                result['xnb_audited_files'] += 1
                if details:
                    details['path'] = str(xnb)
                    result['xnb_candidates'].append(details)
                    result['xnb_theoretical_logical_savings_bytes'] += details['theoretical_logical_savings_bytes']
            except (OSError, ValueError) as exc:
                error(f'{xnb}: {exc}')
    except subprocess.TimeoutExpired:
        result['xnb_audit_status'] = 'timeout'
    except (OSError, ValueError) as exc:
        error(str(exc))
    return result


def atlas_hint(path):
    return bool({'atlas', 'atlases', 'spritesheet', 'spritesheets'}
                & set(re.split('[^a-z0-9]+', str(path).lower())))


def parse_godot4_audit(stdout, source_bytes):
    rows = [line for line in stdout.splitlines() if line.startswith('GODOT_TEXTURE|')]
    if len(rows) != 1:
        raise ValueError(f'expected one GODOT_TEXTURE response, got {len(rows)}')
    fields = rows[0].split('|')
    if len(fields) != 9 or any(not field.isascii() or not field.isdecimal() for field in fields[1:7]):
        raise ValueError('unexpected Godot texture audit schema')
    before, after, entries, changed, skipped, declared_saved = map(int, fields[1:7])
    if before != source_bytes or fields[8] not in ('CANDIDATE', 'NO_GAIN', 'LOW_EFFICIENCY'):
        raise ValueError('Godot texture audit source or status mismatch')
    if fields[8] == 'CANDIDATE' and not (changed > 0 and after < before and declared_saved > 0):
        raise ValueError('Godot texture candidate totals are inconsistent')
    return {'source_bytes': before, 'candidate_bytes': after, 'entries': entries,
            'changed_textures': changed, 'skipped_textures': skipped,
            'estimated_logical_savings_bytes': before - after if fields[8] == 'CANDIDATE' else 0,
            'status': fields[8], 'raw_response': rows[0]}


def render_markdown(report):
    rows = report['games']
    ok = [row for row in rows if row['status'] == 'ok']
    total = sum(row['estimated_logical_savings_bytes'] for row in ok)
    lines = [
        '# Asset opportunity report', '',
        f"Profile: `{report['profile']}`. Backend SHA-256: `{report['backend_sha256']}`.",
        f"Completed: {len(ok)}/{report['selected_count']} games; {len(rows)} rows recorded. "
        f"Estimated logical reduction from completed games: {total:,} bytes.",
        '',
        'Planner estimates are logical file bytes. Physical Btrfs savings and game compatibility are unknown.',
        'Godot 4 PCK texture estimates come from separate read-only audits; Godot 3 PCK reduction is not estimated.',
        'Inventory bytes include unsupported or ineligible content and must not be added to candidate savings.',
        '',
        '| Game | Planner reduction | Godot 4 reduction | Combined logical reduction | Packed inventory | Scan seconds | Status |',
        '|---|---:|---:|---:|---:|---:|---|',
    ]
    for row in sorted(rows, key=lambda item: item.get('estimated_logical_savings_bytes', -1), reverse=True):
        name = row['name'].replace('|', '\\|').replace('\n', ' ')
        if row['status'] == 'ok':
            lines.append(f"| {name} | {row['planner_logical_savings_bytes']:,} | "
                         f"{row['godot4_logical_savings_bytes']:,} | {row['estimated_logical_savings_bytes']:,} | "
                         f"{row['inventory_packed_bytes']:,} | "
                         f"{row['scan_seconds']:.1f} | ok |")
        else:
            lines.append(f"| {name} | — | — | — | — | {row['scan_seconds']:.1f} | {row['status']} |")
    if report.get('experimental_xnb_backend_sha256'):
        lines.extend(['', '## Experimental XNB v5 texture opportunities', '',
                      'These are theoretical payload-byte reductions for audited single-mip BC textures at the selected edge. '
                      'They are excluded from the combined totals above. Partial/error/timeout rows include only candidates recorded so far, not complete coverage. '
                      'Header discovery omits compressed/other-version files. Atlas references, runtime compatibility, visual quality and physical savings are unverified.',
                      '', '| Game | Candidate files | Theoretical logical reduction | Audited / header files | Audit status |',
                      '|---|---:|---:|---:|---|'])
        for row in sorted(rows, key=lambda item: item.get('xnb_theoretical_logical_savings_bytes', 0), reverse=True):
            name = row['name'].replace('|', '\\|').replace('\n', ' ')
            lines.append(f"| {name} | {len(row.get('xnb_candidates', []))} | "
                          f"{row.get('xnb_theoretical_logical_savings_bytes', 0):,} | "
                          f"{row.get('xnb_audited_files', 0)} / {row.get('xnb_header_files', 0)} | "
                         f"{row.get('xnb_audit_status', 'not run')} |")
    return '\n'.join(lines) + '\n'


def atomic_write(path, content):
    with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', prefix='.asset-report-', dir=path.parent,
                                     delete=False) as output:
        temporary = Path(output.name)
        try:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--status-json', required=True, type=Path,
                        help='Saved btrfs-game-compressor --status --json output')
    parser.add_argument('--backend', required=True, type=Path, help='Existing bgc-native executable')
    parser.add_argument('--output-dir', required=True, type=Path)
    parser.add_argument('--profile', default='balanced', choices=[
        'balanced', 'performance', 'ultra-performance', 'quality', 'ultra-quality'])
    parser.add_argument('--timeout', type=int, default=180, help='Seconds allowed per game')
    parser.add_argument('--limit', type=int, default=0, help='First N games only; 0 means all')
    parser.add_argument('--resume', action='store_true', help='Continue an interrupted matching report')
    parser.add_argument('--xnb-backend', type=Path,
                        help='Optional development-audits build for separate, read-only XNB texture opportunities')
    parser.add_argument('--xnb-max-edge', type=int, default=1024, help='Experimental XNB target edge (64..8192)')
    args = parser.parse_args(argv)
    if args.timeout < 1 or args.limit < 0 or not 64 <= args.xnb_max_edge <= 8192:
        parser.error('--timeout must be positive, --limit nonnegative, and --xnb-max-edge 64..8192')
    discovery = json.loads(args.status_json.read_text())
    games = discovery.get('games')
    if not isinstance(games, list):
        parser.error('status JSON has no games list')
    selected = games[:args.limit] if args.limit else games
    for index, game in enumerate(selected, 1):
        if not isinstance(game, dict) or not isinstance(game.get('name'), str) or not isinstance(game.get('path'), str):
            parser.error(f'game {index} has no name or path')
        if args.output_dir.resolve().is_relative_to(Path(game['path']).resolve()):
            parser.error(f'output directory is inside scanned game {game["name"]!r}')
    args.output_dir.mkdir(parents=True, exist_ok=True)
    backend, backend_hash = fixed_backend(args.backend, args.output_dir)
    xnb_backend = None
    xnb_hash = None
    if args.xnb_backend:
        xnb_backend, xnb_hash = fixed_backend(args.xnb_backend, args.output_dir)
        gate = subprocess.run([str(xnb_backend), '--development-audits'],
                              capture_output=True, text=True, timeout=10, check=False)
        if gate.returncode or 'Development reader routes: compiled in' not in gate.stdout:
            parser.error('--xnb-backend must be a development-audits build')
    report_path = args.output_dir / 'asset-opportunities.json'
    markdown_path = args.output_dir / 'asset-opportunities.md'
    source_hash = sha256(args.status_json)
    manifest = [{'name': game['name'], 'path': game['path']} for game in selected]
    if args.resume:
        if not report_path.is_file():
            parser.error('--resume requires an existing asset-opportunities.json')
        report = json.loads(report_path.read_text())
        if (report.get('schema') != 3 or report.get('profile') != args.profile
                or report.get('backend_sha256') != backend_hash
                or report.get('experimental_xnb_backend_sha256') != xnb_hash
                or report.get('xnb_max_edge', 1024) != args.xnb_max_edge
                or report.get('status_json_sha256') != source_hash
                or report.get('selected_games') != manifest):
            parser.error('existing report does not match this backend, status document, profile and game selection')
        if len(report.get('games', [])) > len(selected):
            parser.error('existing report has more results than selected games')
        for recorded, expected in zip(report['games'], manifest):
            if recorded['name'] != expected['name'] or recorded['path'] != expected['path']:
                parser.error('existing report game order differs from selected games')
    else:
        if report_path.exists() or markdown_path.exists():
            parser.error('report already exists; choose a new output directory or pass --resume')
        report = {'schema': 3, 'profile': args.profile, 'backend_sha256': backend_hash,
                  'experimental_xnb_backend_sha256': xnb_hash, 'xnb_max_edge': args.xnb_max_edge,
                  'status_json_sha256': source_hash, 'selected_count': len(selected),
                  'selected_games': manifest, 'started_unix': time.time(),
                  'physical_savings_bytes': None, 'games': []}
    for index in range(len(report['games']) + 1, len(selected) + 1):
        game = selected[index - 1]
        name, path = game.get('name'), game.get('path')
        row = {'name': name, 'path': path, 'status': 'error'}
        started = time.monotonic()
        try:
            result = subprocess.run([str(backend), 'asset-plan', args.profile, path],
                                    capture_output=True, text=True, timeout=args.timeout, check=False)
            if result.returncode:
                row['error'] = f'planner exited {result.returncode}: {result.stderr[-1000:]}'
            else:
                row.update(parse_plan(result.stdout, args.profile))
                row['godot4_pcks'] = []
                row['godot4_logical_savings_bytes'] = 0
                for pck, source_bytes in godot4_pcks(Path(path)):
                    remaining = args.timeout - (time.monotonic() - started)
                    if remaining <= 0:
                        raise subprocess.TimeoutExpired('godot-texture-audit', args.timeout)
                    audited = subprocess.run([str(backend), 'godot-texture-audit', args.profile, str(pck)],
                                             capture_output=True, text=True, timeout=remaining, check=False)
                    if audited.returncode:
                        raise ValueError(f'Godot audit exited {audited.returncode} for {pck}: {audited.stderr[-1000:]}')
                    details = parse_godot4_audit(audited.stdout, source_bytes)
                    details['path'] = str(pck)
                    row['godot4_pcks'].append(details)
                    row['godot4_logical_savings_bytes'] += details['estimated_logical_savings_bytes']
                row['estimated_logical_savings_bytes'] += row['godot4_logical_savings_bytes']
                row['status'] = 'ok'
        except subprocess.TimeoutExpired:
            row['status'] = 'timeout'
            row['error'] = f'exceeded {args.timeout} seconds'
        except (OSError, ValueError) as error:
            row['error'] = str(error)
        if xnb_backend and row['status'] == 'ok':
            row.update(xnb_opportunities(xnb_backend, Path(path), args.xnb_max_edge, started + args.timeout))
        row['scan_seconds'] = time.monotonic() - started
        report['games'].append(row)
        atomic_write(report_path, json.dumps(report, indent=2) + '\n')
        atomic_write(markdown_path, render_markdown(report))
        print(f'[{index}/{len(selected)}] {name}: {row["status"]}', file=sys.stderr, flush=True)
    report['finished_unix'] = time.time()
    atomic_write(report_path, json.dumps(report, indent=2) + '\n')
    atomic_write(markdown_path, render_markdown(report))
    print(markdown_path)
    return 1 if any(row['status'] != 'ok' or row.get('xnb_audit_status', 'ok') != 'ok'
                    for row in report['games']) else 0


if __name__ == '__main__':
    raise SystemExit(main())
