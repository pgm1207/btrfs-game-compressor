#!/usr/bin/env python3
"""Measure real on-disk (compressed) size of a file by writing it fresh and
sampling df used before/after with settling, repeated and taking the median.

Each trial uses a new, owned temporary directory. Never remove a pre-existing
probe directory: it could contain unrelated user data.
"""
import argparse, json, os, subprocess, sys, time, shutil, statistics, tempfile, stat

MOUNT = "/mnt/storage/Games"

def df_used():
    out = subprocess.check_output(["df", "-B1", "--output=used", MOUNT]).decode().splitlines()[-1]
    return int(out.strip())

def clear_cache_sync():
    # sync flushes buffered writes; it does NOT drop the Linux page cache.
    subprocess.run(["sync"], check=True)
    time.sleep(1.5)

def measure_file(src, label, *, level=None, repetitions=3, emit=True):
    """Copy a source to new scratch directories; measure filesystem USED deltas.

    A requested compression property is set on each fresh directory BEFORE
    creating its file. The original source is opened for reading only.
    """
    if repetitions < 1 or repetitions > 10:
        raise ValueError("repetitions must be between 1 and 10")
    if level is not None and level not in range(1, 16):
        raise ValueError("Zstd level must be between 1 and 15")
    deltas = []
    for _ in range(repetitions):
        with tempfile.TemporaryDirectory(prefix=".bgc-probe-", dir=MOUNT) as probe:
            if level is not None:
                # The btrfs tool only operates on our new disposable directory.
                # Failure must abort the trial rather than report an uncompressed
                # result as if the selected compression level had been applied.
                subprocess.run(
                    ["btrfs", "property", "set", probe, "compression", f"zstd:{level}"],
                    check=True, capture_output=True, text=True)
            clear_cache_sync()
            before = df_used()
            shutil.copy2(src, os.path.join(probe, os.path.basename(src)))
            clear_cache_sync()
            after = df_used()
            deltas.append(after - before)
    d = statistics.median(deltas)
    logical = os.path.getsize(src)
    ratio = f"{100*d/logical:5.1f}%" if logical else "n/a"
    if emit:
        policy = f"zstd:{level}" if level is not None else "inherited"
        print(f"{label:34s} policy={policy:9s} logical={logical:>12,}  "
              f"used_delta={int(d):>12,}  ({ratio})")
    return logical, int(d)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scratch-dir", required=True,
                        help="existing writable directory on the target filesystem")
    parser.add_argument("--zstd-level", type=int, choices=range(1, 16),
                        action="append", dest="levels",
                        help="Btrfs Zstd level to measure; repeat for comparisons")
    parser.add_argument("--repetitions", type=int, default=3,
                        help="trials per file and compression level (1-10; default: 3)")
    parser.add_argument("--json", action="store_true", help="emit machine-readable results")
    parser.add_argument("files", nargs="+", help="source files to copy into scratch probes")
    args = parser.parse_args(argv)
    if not 1 <= args.repetitions <= 10:
        parser.error("--repetitions must be between 1 and 10")
    global MOUNT
    MOUNT = os.path.abspath(os.path.expanduser(args.scratch_dir))
    try:
        info = os.lstat(MOUNT)
        if not stat.S_ISDIR(info.st_mode):
            raise ValueError("scratch directory must be a real directory, not a symlink")
        rows = []
        for src in args.files:
            for level in dict.fromkeys(args.levels or [None]):
                logical, used = measure_file(src, os.path.basename(src), level=level,
                                             repetitions=args.repetitions, emit=not args.json)
                rows.append({
                    "source": os.path.abspath(src),
                    "compression": f"zstd:{level}" if level is not None else "inherited",
                    "logical_bytes": logical,
                    "observed_fs_used_delta_bytes": used,
                    "repetitions": args.repetitions,
                })
        if args.json:
            print(json.dumps({
                "schema_version": 1,
                "kind": "compression_probe",
                "scratch_directory": MOUNT,
                "results": rows,
                "caveat": "Filesystem-wide used deltas contain metadata and unrelated "
                          "writes; these are sample observations, not guaranteed game "
                          "savings or runtime performance measurements.",
            }, indent=2))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"measurement failed: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
