#!/usr/bin/env python3
"""Read-only, independently reproducible filesystem-space snapshots for game installs.

Never calls sudo or mutates games. The optional native extent measurement may be
unavailable without privileges; an unavailable value is null, never zero.
"""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile

SCHEMA = 1
MEASURE_KEYS = ("disk_bytes", "raw_extent_bytes", "referenced_bytes")


def _now():
    return datetime.now(timezone.utc).isoformat()


def _measure(root, backend):
    if backend is None:
        return {"status": "unavailable", "reason": "no backend supplied",
                **{key: None for key in MEASURE_KEYS}}
    try:
        child = subprocess.run([str(backend), "measure-bytes", str(root)],
                               capture_output=True, text=True, timeout=120, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"status": "unavailable", "reason": type(error).__name__,
                **{key: None for key in MEASURE_KEYS}}
    if child.returncode != 0:
        return {"status": "unavailable", "reason": "native measurement failed",
                **{key: None for key in MEASURE_KEYS}}
    parts = child.stdout.strip().split("|")
    if len(parts) != 3 or any(not value.isascii() or not value.isdecimal() for value in parts):
        return {"status": "unavailable", "reason": "invalid native protocol",
                **{key: None for key in MEASURE_KEYS}}
    return {"status": "measured", "reason": None,
            **dict(zip(MEASURE_KEYS, map(int, parts)))}


def capture(root, backend=None):
    root = Path(root).absolute()
    metadata = os.lstat(root)
    if not stat.S_ISDIR(metadata.st_mode):
        raise ValueError("Game root must be a real directory, not a symlink")
    vfs = os.statvfs(root)
    block = vfs.f_frsize
    if block <= 0:
        raise ValueError("Filesystem fragment size is invalid")
    return {
        "schema_version": SCHEMA,
        "kind": "snapshot",
        "captured_utc": _now(),
        "root": str(root),
        "device": metadata.st_dev,
        "filesystem_space": {
            "available_bytes": vfs.f_bavail * block,
            "free_bytes": vfs.f_bfree * block,
            "total_bytes": vfs.f_blocks * block,
            "fragment_bytes": block,
        },
        "extent_measurement": _measure(root, backend),
        "caveat": "Point-in-time filesystem-wide values; unrelated writes, "
                  "snapshots, delayed allocation and quotas affect free-space changes.",
    }


def _validate(snapshot):
    if not isinstance(snapshot, dict) or snapshot.get("kind") != "snapshot" or snapshot.get("schema_version") != SCHEMA:
        raise ValueError("Expected a v1 space snapshot")
    fs = snapshot.get("filesystem_space")
    extent = snapshot.get("extent_measurement")
    if not isinstance(fs, dict) or not isinstance(extent, dict):
        raise ValueError("Missing snapshot measurement section")
    for key in ("available_bytes", "free_bytes", "total_bytes", "fragment_bytes"):
        if type(fs.get(key)) is not int or fs[key] < 0:
            raise ValueError("Invalid filesystem measurement")
    if fs["available_bytes"] > fs["free_bytes"] or fs["free_bytes"] > fs["total_bytes"]:
        raise ValueError("Impossible filesystem free-space values")
    if extent.get("status") not in ("measured", "unavailable"):
        raise ValueError("Invalid extent measurement status")
    for key in MEASURE_KEYS:
        value = extent.get(key)
        if extent["status"] == "measured":
            if type(value) is not int or value < 0:
                raise ValueError("Invalid native extent measurement")
        elif value is not None:
            raise ValueError("Unavailable native values must be null")
    if not isinstance(snapshot.get("root"), str) or not snapshot["root"] or type(snapshot.get("device")) is not int:
        raise ValueError("Missing filesystem identity")


def compare(before, after):
    _validate(before)
    _validate(after)
    if before["root"] != after["root"] or before["device"] != after["device"]:
        raise ValueError("Snapshots target different game paths or devices")
    old_space, new_space = before["filesystem_space"], after["filesystem_space"]
    old_extent, new_extent = before["extent_measurement"], after["extent_measurement"]
    measured = old_extent["status"] == new_extent["status"] == "measured"
    return {
        "schema_version": SCHEMA,
        "kind": "comparison",
        "root": before["root"],
        "device": before["device"],
        "before_utc": before.get("captured_utc"),
        "after_utc": after.get("captured_utc"),
        "filesystem_available_delta_bytes": new_space["available_bytes"] - old_space["available_bytes"],
        "filesystem_free_delta_bytes": new_space["free_bytes"] - old_space["free_bytes"],
        "game_extent_disk_reduction_bytes": (old_extent["disk_bytes"] - new_extent["disk_bytes"]) if measured else None,
        "game_raw_extent_reduction_bytes": (old_extent["raw_extent_bytes"] - new_extent["raw_extent_bytes"]) if measured else None,
        "game_referenced_extent_reduction_bytes": (old_extent["referenced_bytes"] - new_extent["referenced_bytes"]) if measured else None,
        "extent_data_status": "measured" if measured else "unavailable",
        "measurement_limitations": [
            "Filesystem deltas measure the whole volume, not savings caused by this tool.",
            "Extent disk bytes represent referenced game data and exclude other filesystem metadata.",
            "Snapshots, reflinks, backups, other writers and delayed allocation can prevent space reclamation.",
            "Negative deltas are genuine reported growth, not clipped to zero.",
        ],
    }


def _write_json(output, obj):
    serialized = json.dumps(obj, indent=2, sort_keys=True) + "\n"
    if output == "-":
        sys.stdout.write(serialized)
    else:
        # Preserve existing reports when writing fails: first fsync a file in a
        # private sibling directory held open by descriptor, then atomically
        # replace the destination from that descriptor. Failed staging
        # directories are deliberately left in place: cleanup by pathname
        # cannot prove that another process did not replace the entry.
        target = Path(output)
        if target.is_symlink():
            raise ValueError("Refusing to overwrite a symlink output")
        directory_flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0)
        parent_fd = os.open(target.parent, directory_flags)
        staging_fd = None
        try:
            staging_path = Path(tempfile.mkdtemp(
                prefix=f".{target.name}.", suffix=".tmp", dir=target.parent))
            created = os.stat(staging_path, follow_symlinks=False)
            staging_fd = os.open(
                staging_path.name,
                directory_flags | getattr(os, "O_NOFOLLOW", 0),
                dir_fd=parent_fd,
            )
            bound = os.fstat(staging_fd)
            visible = os.stat(
                staging_path.name, dir_fd=parent_fd, follow_symlinks=False)
            identities = {
                (created.st_dev, created.st_ino),
                (bound.st_dev, bound.st_ino),
                (visible.st_dev, visible.st_ino),
            }
            if len(identities) != 1 or not stat.S_ISDIR(bound.st_mode):
                raise OSError("Staging directory identity changed during setup")
            descriptor = os.open(
                "report", os.O_WRONLY | os.O_CREAT | os.O_EXCL,
                0o600, dir_fd=staging_fd)
            with os.fdopen(descriptor, "w", encoding="utf-8") as temporary:
                temporary.write(serialized)
                temporary.flush()
                os.fsync(temporary.fileno())
            # Both names are resolved from held directory descriptors. Replacing
            # the public staging-directory name cannot substitute the source.
            os.replace(
                "report", target.name,
                src_dir_fd=staging_fd, dst_dir_fd=parent_fd)
            os.fsync(staging_fd)
            os.fsync(parent_fd)
            # Only remove an empty staging directory after successful
            # publication. A substituted non-empty directory is preserved.
            try:
                os.rmdir(staging_path.name, dir_fd=parent_fd)
            except OSError:
                pass
            else:
                os.fsync(parent_fd)
        finally:
            if staging_fd is not None:
                os.close(staging_fd)
            os.close(parent_fd)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    snap = commands.add_parser("capture", help="Take a read-only filesystem and optional Btrfs extent snapshot")
    snap.add_argument("game", type=Path)
    snap.add_argument("--backend", type=Path, default=None, help="Optional bgc-native binary; no sudo escalation")
    snap.add_argument("--output", default="-", help="JSON filename or - for stdout")
    diff = commands.add_parser("compare", help="Compare two prior snapshots without touching games")
    diff.add_argument("before", type=Path)
    diff.add_argument("after", type=Path)
    diff.add_argument("--output", default="-", help="JSON filename or - for stdout")
    args = parser.parse_args(argv)
    try:
        if args.command == "capture":
            report = capture(args.game, args.backend)
        else:
            report = compare(json.loads(args.before.read_text()), json.loads(args.after.read_text()))
        _write_json(args.output, report)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        parser.exit(1, f"space-delta: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
