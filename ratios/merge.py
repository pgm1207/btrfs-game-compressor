#!/usr/bin/env python3
"""Merge a `--export-ratios` document into ratios/games.json.

Usage:
    ratios/merge.py TARGET INCOMING [--dry-run]

TARGET is normally ratios/games.json; INCOMING is the file written by
`btrfs-game-compressor --export-ratios`. For every (game, zstd level) the incoming
average is folded into the existing row as a sample-weighted mean, the sample count
is summed, and the min/max range is widened. Rows the table does not have yet are
added. TARGET is rewritten in place (atomically) unless --dry-run is given.

This is a contributor tool, not part of the installed runtime; the script itself
stays POSIX awk only. It lives here because merging JSON is exactly the kind of
thing bash is bad at.
"""
import argparse
import datetime
import json
import os
import sys
import tempfile


def number(value, default=0.0):
    try:
        return float(value)
    except (TypeError, ValueError):
        return default


def merge(target, incoming):
    """Fold `incoming` into `target` in place; return (added, updated) counts."""
    games = target.setdefault("games", {})
    added = updated = 0
    for name, levels in (incoming.get("games") or {}).items():
        if not isinstance(levels, dict):
            continue
        for level, row in levels.items():
            if not isinstance(row, dict):
                continue
            pct = number(row.get("pct"))
            samples = int(number(row.get("samples"), 1)) or 1
            low = number(row.get("min"), pct)
            high = number(row.get("max"), pct)

            current = games.get(name, {}).get(level)
            if current is None:
                games.setdefault(name, {})[level] = {
                    "pct": round(pct, 1),
                    "samples": samples,
                    "min": round(low, 1),
                    "max": round(high, 1),
                }
                added += 1
                continue

            current_samples = int(number(current.get("samples"), 1)) or 1
            total = current_samples + samples
            current["pct"] = round(
                (number(current.get("pct")) * current_samples + pct * samples) / total, 1)
            current["samples"] = total
            current["min"] = round(min(number(current.get("min"), pct), low), 1)
            current["max"] = round(max(number(current.get("max"), pct), high), 1)
            updated += 1

    # Deterministic ordering keeps pull-request diffs small and reviewable.
    target["games"] = {
        name: {level: games[name][level] for level in sorted(games[name])}
        for name in sorted(games)
    }
    target["updated"] = datetime.date.today().isoformat()
    return added, updated


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target")
    parser.add_argument("incoming")
    parser.add_argument("--dry-run", action="store_true",
                        help="print the merged table instead of writing it")
    args = parser.parse_args()

    try:
        with open(args.target) as handle:
            target = json.load(handle)
    except (OSError, json.JSONDecodeError) as exc:
        sys.exit(f"{args.target}: {exc}")
    try:
        with open(args.incoming) as handle:
            incoming = json.load(handle)
    except (OSError, json.JSONDecodeError) as exc:
        sys.exit(f"{args.incoming}: {exc}")

    if not (incoming.get("games") or {}):
        sys.exit(f'{args.incoming}: no "games" entries to merge')

    added, updated = merge(target, incoming)
    text = json.dumps(target, indent=2, ensure_ascii=False) + "\n"

    if args.dry_run:
        sys.stdout.write(text)
        print(f"[dry-run] would add {added}, update {updated}", file=sys.stderr)
        return

    directory = os.path.dirname(os.path.abspath(args.target))
    handle, tmp = tempfile.mkstemp(dir=directory, prefix=".games.json.")
    try:
        with os.fdopen(handle, "w") as out:
            out.write(text)
        os.replace(tmp, args.target)
    except Exception:
        os.unlink(tmp)
        raise
    print(f"Merged into {args.target}: {added} new, {updated} updated "
          f"({len(target['games'])} games total)")


if __name__ == "__main__":
    main()
