#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Score decodes against a reference (jt9) per slot (doc/m4-skimmers.md §3.3).

Both inputs are jt9-style lines: `HHMMSS snr dt freq ~  message ...`.
Our lines carry extra columns after the message (37 wide); jt9's may end
in an AP flag (`a1`..`a7`) or `?`.

  score.py REF OURS [-v]
"""
import re, sys
from collections import defaultdict


def parse(path, ours):
    slots = defaultdict(dict)
    for line in open(path):
        if " ~ " not in line:
            continue
        head, msg = line.rstrip("\n").split(" ~ ", 1)
        f = head.split()
        msg = msg[1:38].strip() if ours else re.sub(r"\s+(a\d|\?)\s*$", "", msg.strip()).strip()
        ap = (not ours) and bool(re.search(r"\s(a\d)\s*$", line))
        # Hashed calls: a fresh jt9 run shows <...> where we may know it.
        msg = re.sub(r"<[^>]*>", "<>", msg)
        slots[f[0]][msg] = {"snr": int(float(f[1])), "dt": float(f[2]), "freq": float(f[3]), "ap": ap, "line": line.rstrip()}
    return slots


def main():
    ref, ours = parse(sys.argv[1], False), parse(sys.argv[2], True)
    verbose = "-v" in sys.argv
    found = missed = extra = ap_missed = 0
    miss_snr = defaultdict(int)
    # Only slots the reference has covered (it may still be running).
    for t in sorted(ref):
        r, o = ref.get(t, {}), ours.get(t, {})
        found += len(r.keys() & o.keys())
        for m in r.keys() - o.keys():
            missed += 1
            ap_missed += r[m]["ap"]
            miss_snr[(r[m]["snr"] // 3) * 3] += 1
            if verbose:
                print(f"MISS  {r[m]['line']}")
        for m in o.keys() - r.keys():
            extra += 1
            if verbose:
                print(f"EXTRA {o[m]['line']}")
    total = found + missed
    print(f"reference {total}, found {found} ({100 * found / max(total, 1):.1f} %), missed {missed} ({ap_missed} AP), extra {extra}")
    print("missed by jt9 SNR: " + " ".join(f"{k}:{v}" for k, v in sorted(miss_snr.items())))


if __name__ == "__main__":
    main()
