# SPDX-License-Identifier: GPL-3.0-or-later
"""The music bank for amdata.py: MUSAN's music (openslr.org/17; all of it
Creative Commons or public domain, none non-commercial), resampled
16 → 12 kS/s and appended to one int16 file per split. The mask model
learns noise against programme and can't reproduce a track, so
share-alike and no-derivatives terms (they bind copies and adaptations)
don't keep tracks out; --strict takes only those under CC BY, CC0 or the
public domain (a generative model's bank). Whole artists go to the validation split (one in eight, by a
hash of the name in ANNOTATIONS), or, for an artist with over 20 tracks
(Kevin MacLeod has 160), single tracks. Each output gets a .tsv beside it:
the tracks in it, their licence line and source.

    python3 musicbank.py musan/music music.i16 music-val.i16 [--strict]

A LICENSE file is blocks between lines of '=': a block names its tracks
(music-<src>-NNNN) and a licence line; a leading block with a licence
and no tracks is the licence of blocks that name none (rfm/).
"""
import os
import re
import sys
import zlib

import numpy as np
from scipy.io import wavfile
from scipy.signal import resample_poly

TRACK = re.compile(r"music-[a-z-]+-\d{4}")
LICLINE = re.compile(r"licen|^\s*CC[ 0]|attribution|public domain", re.I)


def kind(line: str) -> str | None:
    """'ok' (CC BY, CC0, public domain), 'no' (SA, ND, NC) or None."""
    u = re.sub(r"[-_]", " ", line.upper())
    if re.search(r"\bNC\b|NON ?COMMERCIAL|\bSA\b|SHARE ?ALIKE|SHARE A LIKE|\bND\b|NO ?DERIV", u):
        return "no"
    if re.search(r"\bCC0\b|PUBLIC DOMAIN|\bBY\b|ATTRIBUTION", u):
        return "ok"
    return None


def licences(d: str) -> dict[str, tuple[str, str, str]]:
    """track → (kind, licence line, source URL) from d/LICENSE."""
    out, default = {}, None
    text = open(os.path.join(d, "LICENSE"), encoding="utf-8", errors="replace").read()
    for i, blk in enumerate(re.split(r"\n=+[ \t]*(?:\n|$)", text)):
        lines = [l.strip() for l in blk.splitlines() if l.strip()]
        ids = TRACK.findall(blk)
        lic = [(k, l) for l in lines if not TRACK.search(l) and LICLINE.search(l) for k in [kind(l)] if k]
        url = next((l for l in lines if l.startswith("http")), "")
        if not ids:
            if i == 0 and lic:
                default = (lic[0][0], lic[0][1], url)
            continue
        if lic:
            k = "no" if any(k == "no" for k, _ in lic) else "ok"
            line = next(l for kk, l in lic if kk == k)
            for t in ids:
                out[t] = (k, line, url)
        elif default:
            for t in ids:
                out[t] = default
    return out


def artists(d: str) -> dict[str, str]:
    out = {}
    for l in open(os.path.join(d, "ANNOTATIONS"), encoding="utf-8", errors="replace"):
        f = l.split()
        if len(f) >= 4:
            out[f[0]] = f[3]
    return out


def load(path: str) -> np.ndarray:
    fs, x = wavfile.read(path)
    assert fs == 16000 and x.dtype == np.int16 and x.ndim == 1, path
    y = resample_poly(x.astype(np.float32), 3, 4)
    return np.clip(np.round(y), -32768, 32767).astype(np.int16)


def main():
    args = [a for a in sys.argv[1:] if a != "--strict"]
    strict = len(args) < len(sys.argv) - 1
    src, out_train, out_val = args[:3]
    take, skip = {"all": [], "train": [], "val": []}, {}
    for d in sorted(os.listdir(src)):
        p = os.path.join(src, d)
        if not os.path.isfile(os.path.join(p, "LICENSE")):
            continue
        lic, art = licences(p), artists(p)
        for f in sorted(os.listdir(p)):
            if not f.endswith(".wav"):
                continue
            t = f[:-4]
            k, line, url = lic.get(t, ("none", "(MUSAN: CC or public domain)", ""))
            if strict and k != "ok":
                skip[k] = skip.get(k, 0) + 1
                continue
            take["all"].append((os.path.join(p, f), t, art.get(t, t), line, url))
    n = {}
    for _, t, a, *_ in take["all"]:
        n[a] = n.get(a, 0) + 1
    for e in take.pop("all"):
        key = e[1] if n[e[2]] > 20 else e[2]
        take["val" if zlib.crc32(key.encode()) % 8 == 0 else "train"].append(e)
    for split, out in (("train", out_train), ("val", out_val)):
        total = 0
        with open(out, "wb") as fo, open(out + ".tsv", "w") as ft:
            ft.write("track\tartist\tlicence\tsource\n")
            for path, t, a, line, url in take[split]:
                x = load(path)
                fo.write(x.tobytes())
                total += len(x)
                ft.write(f"{t}\t{a}\t{line}\t{url}\n")
        print(f"{split}: {len(take[split])} tracks, {total / 12000 / 3600:.2f} h")
    if skip:
        print("left out:", ", ".join(f"{v} {k}" for k, v in sorted(skip.items())))


if __name__ == "__main__":
    main()
