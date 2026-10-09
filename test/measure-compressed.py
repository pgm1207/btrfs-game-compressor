#!/usr/bin/env python3
"""Measure real on-disk (compressed) size of a file by writing it fresh and
sampling df used before/after with settling, repeated and taking the median.

Each trial uses a new, owned temporary directory. Never remove a pre-existing
probe directory: it could contain unrelated user data.
"""
import os, subprocess, sys, time, shutil, statistics, tempfile

MOUNT = "/mnt/storage/Games"

def df_used():
    out = subprocess.check_output(["df", "-B1", "--output=used", MOUNT]).decode().splitlines()[-1]
    return int(out.strip())

def clear_cache_sync():
    subprocess.run(["sync"], check=False)
    time.sleep(1.5)

def measure_file(src, label):
    """Copy src into a fresh, owned directory on the mount; measure df growth."""
    deltas = []
    for _ in range(3):
        with tempfile.TemporaryDirectory(prefix=".bgc-probe-", dir=MOUNT) as probe:
            clear_cache_sync()
            before = df_used()
            shutil.copy2(src, os.path.join(probe, os.path.basename(src)))
            clear_cache_sync()
            after = df_used()
            deltas.append(after - before)
    d = statistics.median(deltas)
    logical = os.path.getsize(src)
    ratio = f"{100*d/logical:5.1f}%" if logical else "n/a"
    print(f"{label:34s} logical={logical:>12,}  stored={int(d):>12,}  ({ratio})")
    return logical, int(d)

def main():
    for path in sys.argv[1:]:
        measure_file(path, os.path.basename(path))

if __name__ == "__main__":
    main()
