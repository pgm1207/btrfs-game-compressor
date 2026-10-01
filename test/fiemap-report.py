#!/usr/bin/env python3
"""Report a Btrfs file's real compression by parsing FIEMAP output.

'encoded' extents are compressed; for those the physical length in the FIEMAP
output is the *on-disk* block count because the kernel reports the compressed
extent length. Unencoded extents are stored raw. We report:
  - logical size
  - sum of on-disk bytes (physical block count * 4096)
  - effective ratio
Requires sudo for FIEMAP on some kernels.
"""
import re, subprocess, sys, os

EXT = re.compile(r'^\s*\d+:\s+(\d+)\.\.\s+(\d+):\s+(\d+)\.\.\s+(\d+):\s+(\d+):\s*(.*)$')

def fiemap(path):
    out = subprocess.run(
        ["sudo", "-n", "--", "filefrag", "-v", path],
        capture_output=True, text=True).stdout
    if not out:
        out = subprocess.run(["filefrag", "-v", path], capture_output=True, text=True).stdout
    return out

def report(path):
    logical = os.path.getsize(path)
    total_phys = 0
    encoded_phys = 0
    raw_phys = 0
    n_enc = n_raw = 0
    for line in fiemap(path).splitlines():
        m = EXT.match(line)
        if not m:
            continue
        blk = int(m.group(5)) * 4096
        flags = m.group(6)
        total_phys += blk
        if "encoded" in flags:
            encoded_phys += blk; n_enc += 1
        else:
            raw_phys += blk; n_raw += 1
    print(f"{os.path.basename(path):36s} logical={logical:>12,}  on-disk={total_phys:>12,}  "
          f"ratio={100*total_phys/logical:5.1f}%  (encoded extents={n_enc}, raw={n_raw})")
    return logical, total_phys

if __name__ == "__main__":
    for p in sys.argv[1:]:
        report(p)
