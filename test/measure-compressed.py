#!/usr/bin/env python3
"""Measure real on-disk (compressed) size of a file by writing it fresh and
sampling df used before/after with settling, repeated and taking the median."""
import os, subprocess, sys, time, shutil, statistics

MOUNT = "/mnt/storage/Games"
PROBE = os.path.join(MOUNT, "_bgc_probe")

def df_used():
    out = subprocess.check_output(["df", "-B1", "--output=used", MOUNT]).decode().splitlines()[-1]
    return int(out.strip())

def clear_cache_sync():
    subprocess.run(["sync"], check=False)
    time.sleep(1.5)

def measure_file(src, label):
    """Copy src into a fresh dir on the mount, measure df growth."""
    deltas = []
    for _ in range(3):
        shutil.rmtree(PROBE, ignore_errors=True)
        os.makedirs(PROBE, exist_ok=True)
        clear_cache_sync()
        before = df_used()
        shutil.copy2(src, os.path.join(PROBE, os.path.basename(src)))
        clear_cache_sync()
        after = df_used()
        deltas.append(after - before)
        shutil.rmtree(PROBE, ignore_errors=True)
    d = statistics.median(deltas)
    logical = os.path.getsize(src)
    print(f"{label:34s} logical={logical:>12,}  stored={int(d):>12,}  ({100*d/logical:5.1f}%)")
    return logical, int(d)

def main():
    for path in sys.argv[1:]:
        measure_file(path, os.path.basename(path))
    shutil.rmtree(PROBE, ignore_errors=True)

if __name__ == "__main__":
    main()
