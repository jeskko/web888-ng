# SPDX-License-Identifier: GPL-3.0-or-later
"""Clean signals of every class for the signal identifier: complex baseband
at FS, the signal's band centred on 0 Hz, unit power while on (silence
where the mode pauses). Each generator gives at least `n` samples, from a
random point of a transmission.

Content is random where it doesn't identify the mode (FT8's data tones,
PSK modems' data symbols, an image's pixels) and exact where it does (sync
arrays, frame structure, probe and scrambler sequences, baud rates,
shifts, tone spacings). STANAG 4285 and MIL-STD-188-110A frame structure
after datarx's `wahrx/s4285.py` and `wahrx/m110a.py` (MIT, by this
project's author); ALE's tone set after `wahrx/ale.py`.
"""
from __future__ import annotations

import importlib.util
import os

import numpy as np
from scipy.signal import resample_poly, sosfilt, butter, hilbert

# the CW trainer's Morse table
_spec = importlib.util.spec_from_file_location("cw_common", os.path.join(os.path.dirname(__file__), "..", "cw", "common.py"))
_cw = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_cw)
MORSE = _cw.MORSE

FS = 6000.0
TAU = 2 * np.pi


def fm(freq: np.ndarray, rng) -> np.ndarray:
    """exp(j phase) of an instantaneous frequency track (Hz)."""
    return np.exp(1j * (TAU * np.cumsum(freq) / FS + rng.uniform(0, TAU))).astype(np.complex64)


def hold(values: np.ndarray, rate: float, n: int, offset: float = 0.0) -> np.ndarray:
    """Sample-and-hold of per-symbol values at `rate` symbols/s."""
    k = np.floor((np.arange(n) / FS + offset) * rate).astype(int)
    return values[np.clip(k, 0, len(values) - 1)]


def gauss_track(tones: np.ndarray, baud: float, spacing: float, bt: float, n0: int) -> np.ndarray:
    """GFSK frequency track (Hz) of tone numbers: rectangular pulses
    smoothed by a Gaussian of bandwidth-time `bt`."""
    sps = FS / baud
    n = int(len(tones) * sps)
    # at 1/8 of the rate (smooth), interpolated
    m = n // 8 + 1
    f = tones.astype(float)[np.clip((np.arange(m) * 8 / sps).astype(int), 0, len(tones) - 1)] * spacing
    sigma = np.sqrt(np.log(2)) / (TAU * bt) * sps / 8
    k = np.arange(-int(4 * sigma) - 1, int(4 * sigma) + 2)
    g = np.exp(-0.5 * (k / sigma) ** 2)
    f = np.convolve(np.pad(f, len(k), mode="edge"), g / g.sum(), mode="same")[len(k):-len(k)]
    return np.interp(np.arange(n), np.arange(m) * 8, f)


def window(x: np.ndarray, n: int, rng, start=None) -> np.ndarray:
    """n samples of x from a random point (zero-padded)."""
    if len(x) < n:
        x = np.r_[x, np.zeros(n - len(x), np.complex64)]
    a = rng.integers(0, len(x) - n + 1) if start is None else start
    return x[a:a + n]


def bursts(x: np.ndarray, n: int, rng, period=None) -> np.ndarray:
    """A transmission of x somewhere in (or across) an n-sample window,
    silence around it; with `period` (s), repeated as in timed cycles."""
    out = np.zeros(n, np.complex64)
    if period is None:
        a = rng.integers(-len(x) + n // 2, n // 2)
        lo, hi = max(a, 0), min(a + len(x), n)
        out[lo:hi] = x[lo - a:hi - a]
        return out
    p = int(period * FS)
    a = rng.integers(-p, 0)
    while a < n:
        lo, hi = max(a, 0), min(a + len(x), n)
        if hi > lo:
            out[lo:hi] = x[lo - a:hi - a]
        a += p
    return out


# --- the classes --------------------------------------------------------------------------------
def noise(n, rng):
    return np.zeros(n, np.complex64)


def carrier(n, rng):
    drift = rng.uniform(-2, 2) * np.arange(n) / n
    return fm(drift, rng)


def cw(n, rng):
    wpm = float(np.exp(rng.uniform(np.log(10), np.log(40))))
    u = 1.2 / wpm
    jit = rng.choice([0.0, 0.05, 0.12])
    chars = [c for c in MORSE if len(c) == 1]
    key = []
    while len(key) < 2 * n:
        for _ in range(rng.integers(1, 7)):
            for i, e in enumerate(MORSE[chars[rng.integers(len(chars))]]):
                d = (1 if e == "." else 3) * u * max(0.3, 1 + jit * rng.normal())
                key += [1.0] * int(d * FS) + [0.0] * int(u * FS * max(0.3, 1 + jit * rng.normal()))
            key += [0.0] * int(2 * u * FS)
        key += [0.0] * int((4 * u if rng.random() > 0.1 else rng.uniform(0.5, 2.0)) * FS)
    key = np.array(key, np.float32)
    m = max(1, int(rng.uniform(0.002, 0.008) * FS))
    w = np.hanning(2 * m + 1)
    key = np.convolve(key, w / w.sum(), mode="same")
    return window(key.astype(np.complex64), n, rng) * fm(np.zeros(n), rng)


def fsk(n, rng):
    """2-FSK: async RTTY/ASCII (start, data, parity?, stop) or synchronous
    (SITOR-B / NAVTEX: 100 Bd, 7-bit constant-ratio)."""
    kind = rng.choice(["rtty", "rtty", "rtty", "ascii", "sitor", "dwd"])
    if kind == "rtty":
        baud, shift = rng.choice([45.45, 45.45, 50.0, 75.0]), rng.choice([170.0, 170.0, 85.0, 200.0, 425.0, 850.0])
        ndata, stop = 5, rng.choice([1.5, 1.0, 2.0])
    elif kind == "dwd":
        baud, shift, ndata, stop = 50.0, 450.0, 5, 1.5
    elif kind == "ascii":
        baud, shift = rng.choice([110.0, 150.0, 300.0]), rng.choice([170.0, 200.0, 850.0])
        ndata, stop = rng.choice([7, 8]), 1.0
    else:
        baud, shift = 100.0, 170.0
    halves = []                                        # in half bits (1.5 stop bits)
    if kind == "sitor":
        weights = np.array([b for b in range(128) if bin(b).count("1") == 4])
        for c in rng.choice(weights, int(n / FS * baud / 7) + 20):
            halves += [(c >> i) & 1 for i in range(7) for _ in range(2)]
    else:
        idle = rng.random() < 0.3
        while len(halves) < 2 * n / FS * baud * 2:
            c = rng.integers(0, 1 << ndata)
            b = [0] + [(c >> i) & 1 for i in range(ndata)]
            if kind == "ascii" and ndata == 7 and rng.random() < 0.5:
                b.append(bin(c).count("1") & 1)
            halves += [v for v in b for _ in range(2)] + [1] * int(round(2 * stop))
            if idle and rng.random() < 0.2:
                halves += [1] * rng.integers(2, 40)
    h = np.array(halves, float)
    f = hold(np.where(h > 0, shift / 2, -shift / 2), 2 * baud, 2 * n)
    return window(fm(f, rng), n, rng)


VARICODE = None


def _varicode():
    # PSK31 varicode: codes with no '00' inside, starting and ending in 1 (all such words, shortest first)
    global VARICODE
    if VARICODE is None:
        out = []
        for L in range(1, 11):
            for v in range(1 << L):
                s = format(v, f"0{L}b")
                if s[0] == "1" and s[-1] == "1" and "00" not in s:
                    out.append(s)
        VARICODE = out[:128]
    return VARICODE


def psk31(n, rng):
    """BPSK 31.25 (sometimes 63 / 125) Bd: a reversal for each 0 bit, the
    raised-cosine amplitude dip at each; varicode characters, idle zeros."""
    baud = rng.choice([31.25, 31.25, 31.25, 62.5, 125.0])
    vc = _varicode()
    bits = []
    nb = int(2 * n / FS * baud) + 10
    idle = rng.random() < 0.15
    while len(bits) < nb:
        if idle or rng.random() < 0.05:
            bits += [0] * rng.integers(4, 30)
        else:
            bits += [int(b) for b in vc[min(int(rng.exponential(20)), 127)]] + [0, 0]
    bits = np.array(bits[:nb])
    sym = np.cumprod(np.where(bits == 0, -1.0, 1.0))
    sps = FS / baud
    t = np.arange(int(nb * sps)) / sps                   # in symbols
    k = np.floor(t).astype(int)
    frac = t - k
    a, b = sym[np.clip(k, 0, nb - 1)], sym[np.clip(k + 1, 0, nb - 1)]
    # the transition at each symbol boundary: cosine from a to b over the symbol
    env = a * (0.5 + 0.5 * np.cos(np.pi * frac)) + b * (0.5 - 0.5 * np.cos(np.pi * frac))
    x = env.astype(np.complex64) * fm(np.zeros(len(env)), rng)
    return window(x, n, rng)


COSTAS8 = np.array([3, 1, 4, 0, 6, 5, 2])
COSTAS4 = [np.array(v) for v in ([0, 1, 3, 2], [1, 0, 2, 3], [2, 3, 1, 0], [3, 2, 0, 1])]


def ft8_one(rng):
    t = rng.integers(0, 8, 79)
    for a in (0, 36, 72):
        t[a:a + 7] = COSTAS8
    f = gauss_track(t, 6.25, 6.25, 2.0, 0) - 3.5 * 6.25
    return fm(f, rng)


def ft4_one(rng):
    d = [rng.integers(0, 4, 29) for _ in range(3)]
    t = np.concatenate([COSTAS4[0], d[0], COSTAS4[1], d[1], COSTAS4[2], d[2], COSTAS4[3]])
    f = gauss_track(t, 20.833, 20.833, 1.0, 0) - 1.5 * 20.833
    return fm(f, rng)


def _crowd(one, n, rng, period, width, others):
    """The centred signal plus others of the mode at other audio offsets,
    all in the same timed cycles (a busy FT8/FT4 sub-band)."""
    x = bursts(one(rng), n, rng, period)
    a = rng.integers(0, int(period * FS))
    for _ in range(rng.integers(0, others + 1)):
        off = rng.choice([-1, 1]) * rng.uniform(width + 10, 2900)
        lvl = 10 ** rng.uniform(-1.5, 0.5)
        y = bursts(one(rng), n + int(period * FS), rng, period)[a:a + n]
        x = x + lvl * y * np.exp(1j * TAU * off * np.arange(n) / FS).astype(np.complex64)
    return x


def ft8(n, rng):
    return _crowd(ft8_one, n, rng, 15.0, 50, 6)


def ft4(n, rng):
    return _crowd(ft4_one, n, rng, 7.5, 83, 5)


def wspr(n, rng):
    def one(rng):
        # the symbols around a window (the transmission is 110.6 s)
        t = rng.integers(0, 4, 12)
        return fm(hold(t * 1.4648 - 2.2, 1.4648, n, offset=rng.uniform(0, 0.683)), rng)
    x = one(rng)
    for _ in range(rng.integers(0, 6)):
        off = rng.choice([-1, 1]) * rng.uniform(10, 100)
        x = x + 10 ** rng.uniform(-1.5, 0.5) * one(rng) * np.exp(1j * TAU * off * np.arange(n) / FS)
    return x.astype(np.complex64)


def ale(n, rng):
    """ALE 2G: 8-FSK 125 Bd, tones 750-2500 Hz (centre 1625); words of 49
    symbols, each sent three times in a call (random tribits here)."""
    nw = int(n / FS * 125 / 49) + 2
    tones = np.concatenate([np.tile(rng.integers(0, 8, 49), 1) for _ in range(nw)])
    x = fm(hold(-875.0 + 250.0 * tones, 125.0, len(tones) * 48), rng)
    return bursts(x, n, rng) if rng.random() < 0.3 else window(x, n, rng)


# serial-tone PSK modems: 2400 Bd on 1800 Hz, RRC 0.35, 4 samples a symbol then to FS
def _rrc(beta=0.35, sps=4, span=8):
    t = np.arange(-span * sps, span * sps + 1) / sps
    den = 1 - (2 * beta * t) ** 2
    h = np.sinc(t) * np.cos(np.pi * beta * t) / np.where(np.abs(den) < 1e-8, 1e-8, den)
    h[np.abs(den) < 1e-8] = np.pi / 4 * np.sinc(1 / (2 * beta))
    return h / np.sqrt((h ** 2).sum())


def _psk_wave(sym: np.ndarray, rng) -> np.ndarray:
    up = np.zeros(len(sym) * 4, np.complex64)
    up[::4] = np.exp(1j * np.pi / 4 * sym)
    x = np.convolve(up, _rrc(), mode="same") * 2.0
    y = resample_poly(x, 5, 8)                          # 9600 -> 6000 S/s
    return (y * np.exp(1j * rng.uniform(0, TAU))).astype(np.complex64)


def _lfsr_4285():
    reg, sync = [1, 1, 0, 1, 0], []
    for _ in range(80):
        sync.append(reg[4])
        reg = [reg[4] ^ reg[2]] + reg[:4]
    reg, scr = [1] * 9, []
    for _ in range(176):
        scr.append(4 * reg[2] + 2 * reg[1] + reg[0])
        for _ in range(3):
            reg = reg[1:] + [reg[0] ^ reg[4]]
    return np.array(sync), np.array(scr)


S4285_SYNC, S4285_SCR = _lfsr_4285()
S4285_REF = np.r_[112:128, 160:176, 208:224]
S4285_DATA = np.r_[80:112, 128:160, 176:208, 224:256]


def stanag4285(n, rng):
    nf = int(n / FS / 0.10667) + 3
    order = rng.choice([2, 4, 8])
    fr = np.zeros((nf, 256), int)
    fr[:, :80] = 4 * S4285_SYNC
    fr[:, S4285_REF] = S4285_SCR[S4285_REF - 80]
    fr[:, S4285_DATA] = (rng.integers(0, order, (nf, 128)) * (8 // order) + S4285_SCR[S4285_DATA - 80]) % 8
    return window(_psk_wave(fr.reshape(-1), rng), n, rng)


def _scr_110a():
    s = [1, 0, 1, 1, 0, 1, 0, 1, 1, 1, 0, 1]
    out = []
    for _ in range(160):
        for _ in range(8):
            fb = s[11]
            s = [fb, s[0] ^ fb, s[1], s[2], s[3] ^ fb, s[4], s[5] ^ fb, s[6], s[7], s[8], s[9], s[10]]
        out.append(4 * s[2] + 2 * s[1] + s[0])
    return np.array(out)


M110_SCR = _scr_110a()
_PAT = {0: "00000000", 1: "04040404", 2: "00440044", 3: "04400440", 4: "00004444", 5: "04044040",
        6: "00444400", 7: "04404004"}
M110_PAT = {k: np.array([int(c) for c in v * 4]) for k, v in _PAT.items()}
M110_SYNC = np.array([7, 4, 3, 0, 5, 1, 5, 0, 2, 2, 1, 1, 5, 7, 4, 3, 5, 0, 2, 6, 2, 1, 6, 2, 0, 0, 5, 0, 5, 2, 6, 6])


def mil110a(n, rng):
    """Data frames (unknown + probe symbols, all on the 160-symbol
    scrambler), sometimes from the preamble (200 ms segments)."""
    bps = rng.choice([4800, 2400, 2400, 1200, 600, 300, 150])
    nu, nk = (32, 16) if bps >= 2400 else (20, 20)
    order = 8 if bps >= 2400 else 4 if bps == 1200 else 2
    nsym = int(n / FS * 2400) + 2400
    k = np.arange(nsym)
    p = k % (nu + nk)
    sym = np.where(p >= nu, 0, rng.integers(0, order, nsym) * (8 // order))
    sym = (sym + M110_SCR[k % 160]) % 8
    if rng.random() < 0.3:
        d1, d2 = rng.integers(4, 8, 2)
        segs = []
        for c in range(rng.choice([3, 24]) - 1, -1, -1):
            ch = [0, 1, 3, 0, 1, 3, 1, 2, 0, d1, d2, 4 + ((c >> 4) & 3), 4 + ((c >> 2) & 3), 4 + (c & 3), 0]
            segs.append((np.concatenate([M110_PAT[v] for v in ch]) + np.tile(M110_SYNC, 15)) % 8)
        sym = np.r_[np.concatenate(segs), sym]
        x = _psk_wave(sym, rng)
        a = rng.integers(0, len(segs) * 480 * 5 // 2)          # start within the preamble
        return window(x, n, rng, start=min(a, len(x) - n))
    return window(_psk_wave(sym, rng), n, rng)


# --- images: SSTV and fax ------------------------------------------------------------------------
def _image(h: int, w: int, rng, chart=False) -> np.ndarray:
    """A random picture (h, w, 3) in 0..1: gradients, blobs, stripes, noise;
    or (chart) white with black lines, contours and text-like rows."""
    y, x = np.mgrid[0:h, 0:w] / max(h, w)
    if chart:
        img = np.ones((h, w))
        for _ in range(rng.integers(3, 12)):
            a, b, c = rng.normal(size=3)
            ph = rng.uniform(0, 10)
            img[np.abs(np.sin(a * 20 * x + b * 20 * y + ph) + c * 0.1) < 0.03] = 0
        for _ in range(rng.integers(0, 8)):
            r0, c0 = rng.integers(0, h - 8), rng.integers(0, w - 200)
            img[r0:r0 + 6, c0:c0 + rng.integers(20, 200)] = rng.random((6, 1)) < 0.5
        return np.repeat(img[:, :, None], 3, 2)
    img = np.zeros((h, w, 3))
    for c in range(3):
        img[:, :, c] = rng.uniform(0, 1) + rng.normal() * x + rng.normal() * y
        for _ in range(rng.integers(0, 6)):
            cx, cy, r = rng.uniform(0, 1), rng.uniform(0, 1), rng.uniform(0.05, 0.4)
            img[:, :, c] += rng.normal() * np.exp(-((x - cx) ** 2 + (y - cy) ** 2) / r ** 2)
    img += rng.uniform(0, 0.2) * rng.normal(size=img.shape)
    return np.clip(img, 0, 1)


SSTV = [  # (period ms, sync ms, scans [(start from the sync, duration, channel)])
    (446.446, 4.862, [(5.434, 146.432, 0), (152.438, 146.432, 1), (299.442, 146.432, 2)]),     # Martin 1
    (226.798, 4.862, [(5.434, 73.216, 0), (79.222, 73.216, 1), (153.01, 73.216, 2)]),        # Martin 2
    (428.22, 9.0, [(-277.98, 138.24, 0), (-138.24, 138.24, 1), (10.5, 138.24, 2)]),          # Scottie 1
    (277.692, 9.0, [(-177.628, 88.064, 0), (-88.064, 88.064, 1), (10.5, 88.064, 2)]),        # Scottie 2
    (1050.3, 9.0, [(-692.7, 345.6, 0), (-345.6, 345.6, 1), (10.5, 345.6, 2)]),               # Scottie DX
    (150.0, 9.0, [(12.0, 88.0, 0), (106.0, 44.0, 1)]),                                       # Robot 36
    (300.0, 9.0, [(12.0, 138.0, 0), (156.0, 69.0, 1), (231.0, 69.0, 2)]),                    # Robot 72
] + [(22.08 + 4 * t, 20.0, [(22.08 + i * t, t, i % 3) for i in range(4)])                  # PD 50-240
     for t in (91.52, 170.24, 121.6, 195.584, 183.04, 244.48)]


def sstv(n, rng):
    period, sync, scans = SSTV[rng.integers(len(SSTV))]
    total = 2 * n + int(1.0 * FS)
    t = np.arange(total) / FS * 1000.0                 # ms
    f = np.full(total, 1500.0)
    img = _image(256, 320, rng)
    # VIS header now and then: leaders 1900, break 1200, start bit, 8 bits 1100/1300, stop
    vis = rng.random() < 0.2
    t0 = 0.0
    if vis:
        hdr = [(300, 1900), (10, 1200), (300, 1900), (30, 1200)] + \
              [(30, 1100 if b else 1300) for b in rng.integers(0, 2, 8)] + [(30, 1200)]
        for d, fr in hdr:
            f[(t >= t0) & (t < t0 + d)] = fr
            t0 += d
    tl = t - t0
    line = np.floor(tl / period).astype(int)
    ph = tl - line * period                            # ms since this line's sync
    on = tl >= 0
    f[on & (ph < sync)] = 1200.0
    row = np.clip(line, 0, 255)
    for st, d, ch in scans:
        a = (ph - (st % period)) % period
        m = on & (a < d)
        col = np.clip((a[m] / d * 320).astype(int), 0, 319)
        f[m] = 1500.0 + 800.0 * img[row[m], col, ch]
    if not vis:
        f = f[rng.integers(0, total - n):]
    return window(fm(f[:2 * n] - 1700.0, rng), n, rng, start=0)


def fax(n, rng):
    """HF fax: FM 1500 (black) - 2300 Hz (white), 60/90/120 lines a
    minute; mostly image lines, sometimes the phasing lines or the start
    (300 Hz) / stop (450 Hz) tones."""
    lpm = rng.choice([120, 120, 90, 60])
    lp = 60.0 / lpm
    t = np.arange(n) / FS + rng.uniform(0, 100)
    part = rng.random()
    if part < 0.08:
        tone = rng.choice([300.0, 450.0])
        lum = (np.floor(t * 2 * tone) % 2).astype(float)
    elif part < 0.2:
        lum = np.where((t % lp) / lp < 0.05, 0.0, 1.0)
    else:
        img = _image(16, 1809, rng, chart=rng.random() < 0.7)[:, :, 0]
        line = (np.floor(t / lp) % 16).astype(int)
        col = ((t % lp) / lp * 1809).astype(int)
        lum = img[line, col]
    return fm(1500.0 + 800.0 * lum - 1900.0, rng)


# --- speech: SSB voice and AM broadcast ----------------------------------------------------------
_BANK = None


def speech(n, rng) -> np.ndarray:
    """n samples of real audio at FS from the espeak bank (pauses kept),
    several utterances end to end."""
    global _BANK
    if _BANK is None:
        z = np.load(os.environ.get("SIGID_VOICES", "voices.npz"))
        a, lens = z["audio"].astype(np.float32), z["lens"]
        _BANK = np.split(a, np.cumsum(lens)[:-1])
    out = []
    while sum(len(o) for o in out) < 2 * n:
        c = _BANK[rng.integers(len(_BANK))]
        out += [c * rng.uniform(0.5, 1.0), np.zeros(int(rng.exponential(0.3) * FS), np.float32)]
    x = np.concatenate(out)
    a = rng.integers(0, len(x) - n)
    return x[a:a + n]


def _tilt(x, rng):
    """Speech as it is on air: most of its energy low (a first-order
    lowpass at 300-1200 Hz, partly mixed back), now and then flat."""
    if rng.random() < 0.2:
        return x
    fc = rng.uniform(300, 1200)
    low = sosfilt(butter(1, fc, "lowpass", fs=FS, output="sos"), x)
    a = rng.uniform(0.0, 0.4)
    y = (1 - a) * low / (np.std(low) + 1e-9) + a * x / (np.std(x) + 1e-9)
    return y / (np.abs(y).max() + 1e-9)


def _band(x, lo, hi):
    return sosfilt(butter(6, [lo, hi], "bandpass", fs=FS, output="sos"), x)


def _compress(x, rng):
    if rng.random() < 0.5:
        g = rng.uniform(1, 6)
        x = np.tanh(g * x / (np.abs(x).max() + 1e-9)) / np.tanh(g)
    return x


def ssb_voice(n, rng):
    """Speech 300-2700 Hz as one sideband (USB), centred at 1500 Hz."""
    a = _compress(_band(_tilt(speech(n + 600, rng), rng), rng.uniform(150, 400), rng.uniform(2400, 2900)), rng)[600:]
    z = hilbert(a)
    return (z * np.exp(-1j * TAU * 1500.0 * np.arange(n) / FS)).astype(np.complex64)


def music(n, rng):
    t = np.arange(n) / FS
    out = np.zeros(n)
    beat = rng.uniform(0.15, 0.6)
    k = 0.0
    while k < n / FS:
        f0 = 110 * 2 ** (rng.integers(0, 36) / 12)
        d = beat * rng.integers(1, 4)
        env = np.exp(-np.clip(t - k, 0, None) / rng.uniform(0.1, 1.0)) * (t >= k) * (t < k + 3 * d)
        for h in range(1, rng.integers(2, 8)):
            out += env * np.sin(TAU * f0 * h * t) / h
        if rng.random() < 0.4:
            out += (t >= k) * np.exp(-np.clip(t - k, 0, None) / 0.03) * rng.normal(size=n) * 0.5
        k += d
    return out / (np.abs(out).max() + 1e-9)


def am(n, rng):
    """AM broadcast: carrier and both sidebands of speech or music (the
    window is narrower than the 9-10 kHz channel)."""
    a = _tilt(speech(n, rng), rng) if rng.random() < 0.6 else music(n, rng)
    a = _band(a, 50, 2950)
    # broadcast audio is processed hard: RMS a third of full scale, peaks clipped
    a = np.clip(a / (3 * np.sqrt(np.mean(a ** 2)) + 1e-9), -1, 1)
    m = rng.uniform(0.5, 1.0)
    x = ((1 + m * a) / (1 + m)).astype(np.complex64) * fm(np.zeros(n), rng)
    if rng.random() < 0.4:
        # heard in SSB: the carrier and one sideband (the passband's edge
        # near the carrier)
        X = np.fft.fft(x)
        f = np.fft.fftfreq(n, 1 / FS)
        edge = rng.uniform(-150, -20)
        X[f < edge] *= 1e-3
        x = np.fft.ifft(X).astype(np.complex64)
    return x


CLASSES = ["noise", "carrier", "cw", "fsk", "psk31", "ft8", "ft4", "wspr", "ale", "stanag4285",
           "mil110a", "sstv", "fax", "ssb", "am"]
GEN = [noise, carrier, cw, fsk, psk31, ft8, ft4, wspr, ale, stanag4285, mil110a, sstv, fax, ssb_voice, am]
# half the occupied width of each (Hz): how far a neighbour must stay
HALF = [0, 5, 20, 450, 60, 300, 300, 100, 950, 1600, 1600, 650, 450, 1300, 2950]
