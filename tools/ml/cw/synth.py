# SPDX-License-Identifier: GPL-3.0-or-later
"""Training data for the neural CW decoder, made on the fly: text (QSO
phrases, callsigns, numbers, words, random characters), keying (speed,
hand-keying jitter, dash/dot ratio, Farnsworth gaps), and the channel
(carrier offset and drift, edge shaping, Rayleigh fading: slow QSB or
auroral flutter of 3-30 Hz spread; another CW
signal nearby, or a pile-up: 1-3 callers 12-120 Hz away at similar or
higher level, keyed over the wanted one; impulse noise, white noise at an
SNR in 2500 Hz). The label is only the signal at the centre. Each
example is a WINDOW-second channel at 500 S/s; characters cut by the
window's edges are keyed up, so the label is exactly what is audible.

Independent of the Rust synthesiser (crates/cw/src/synth.rs) on purpose:
the comparison with the classical decoder uses that one.
"""
import random

import numpy as np
import torch

from common import FS, INDEX, MORSE

WINDOW = 12.0

WORDS = ("CQ DE TEST K KN BK R RR TU 5NN 599 579 559 RST UR NAME QTH RIG ANT PWR WX HR ES FB OM GM GE GA "
         "TNX TKS FER QSO 73 72 GL HPE CU AGN PSE QRZ QRL QSY QRM QRN QSB QRP DX POTA SOTA UP BURO LOTW CPY "
         "SRI NR WID VY GD DR OP YAGI DIPOLE VERT WIRE KW W WATTS SUNNY CLOUDY RAIN SNOW TEMP C F HW ? = "
         "THE AND FOR WITH YOU ALL ARE NOT BUT ONE GOOD NICE DAY NIGHT HOME WORK TIME BAND CONDX OK SO "
         "MY IS AT IN ON TO OF BE NOW HERE THERE BACK NEW OLD").split()
NAMES = "JOHN BOB TOM JIM BILL MIKE PETE HANS KARL JUHA PEKKA ERIK OLE JAN ALEX IVAN ANNA MARIA".split()
CITIES = "OULU HELSINKI BERLIN MUNICH LONDON PARIS ROMA MADRID OSLO RIGA TALLINN WIEN PRAHA KYIV".split()
ALL = [c for c in MORSE if len(c) == 1]


def callsign(r: random.Random) -> str:
    L = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    pre = r.choice([r.choice(L), r.choice(L) + r.choice(L), r.choice(L) + str(r.randrange(10))])
    s = pre + str(r.randrange(10)) + "".join(r.choice(L) for _ in range(r.choice((1, 2, 2, 3, 3))))
    if r.random() < 0.1:
        s += "/" + r.choice(["P", "M", "QRP", str(r.randrange(10))])
    return s


def text(r: random.Random) -> str:
    """A line of CW traffic, ~60 characters."""
    k = r.random()
    a, b = callsign(r), callsign(r)
    if k < 0.25:
        t = r.choice([
            f"CQ CQ DE {a} {a} K", f"CQ CQ CQ DE {a} {a} {a} PSE K", f"CQ TEST {a} {a}", f"{b} DE {a} K",
            f"{a} 5NN {r.randrange(1, 40)} TU", f"TU {a} TEST", f"CQ POTA DE {a} {a} K", f"QRZ? DE {a}",
        ])
        t += " " + r.choice([f"{b} DE {a} {a} K", f"{a} TU 5NN", "TU", f"R R {a}", ""])
    elif k < 0.5:
        t = r.choice([
            f"{b} DE {a} GM OM TNX FER CALL UR RST {r.choice(['599', '579', '559', '449'])} "
            f"{r.choice(['599', '579'])} NAME {r.choice(NAMES)} QTH {r.choice(CITIES)} HW? {b} DE {a} KN",
            f"R R TNX FER RPRT {r.choice(NAMES)} RIG HR {r.choice(['IC7300', 'K3', 'FT991', 'TS590'])} "
            f"PWR {r.randrange(5, 1000)}W ANT {r.choice(['DIPOLE', 'YAGI', 'VERT', 'EFHW'])} WX "
            f"{r.choice(['SUNNY', 'CLOUDY', 'RAIN', 'SNOW'])} TEMP {r.randrange(-20, 35)}C = HW?",
            f"TNX FER QSO {r.choice(NAMES)} 73 ES GL CU AGN {b} DE {a} <SK>",
        ])
    elif k < 0.75:
        t = " ".join(r.choice(WORDS + NAMES + CITIES + [callsign(r), str(r.randrange(1000))]) for _ in range(r.randrange(6, 14)))
    else:
        # random characters and groups: no language model to lean on
        words = []
        for _ in range(r.randrange(5, 12)):
            words.append("".join(r.choice(ALL if r.random() < 0.3 else "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")
                                 for _ in range(r.randrange(1, 7))))
        t = " ".join(words)
    return " ".join(t.split())


def chars_of(word: str) -> list[str]:
    out, i = [], 0
    while i < len(word):
        if word[i] == "<":
            j = word.index(">", i)
            out.append(word[i:j + 1])
            i = j + 1
        else:
            out.append(word[i])
            i += 1
    return out


def keying(t: str, r: random.Random, wpm: float, jitter: float, dash: float, fw: float):
    """Marks as (start s, end s, character index) and characters as (start
    s, end s, label)."""
    u = 1.2 / wpm
    uf = 1.2 / fw
    j = (lambda: max(0.25, 1 + jitter * r.gauss(0, 1))) if jitter else (lambda: 1.0)
    t0 = 0.0
    marks, chars = [], []
    for wi, word in enumerate(t.split()):
        if word == "|":
            # a pause between overs or phrases (listening, the other side
            # sending): a space too, however long
            if chars and chars[-1][2] == " ":
                a, _, _ = chars.pop()
                t0 = a
            chars.append((t0, t0 + r.uniform(0.5, 4.0), " "))
            t0 = chars[-1][1]
            continue
        if wi and not (chars and chars[-1][2] == " " and chars[-1][1] == t0):
            # a word gap; now and then a longer pause mid-line
            gap = 7 * uf * j() if r.random() > 0.05 else r.uniform(10 * uf, 1.5)
            chars.append((t0, t0 + gap, " "))
            t0 = chars[-1][1]
        for ci, ch in enumerate(chars_of(word)):
            if ci:
                t0 += 3 * uf * j()
            code = MORSE.get(ch)
            if code is None:
                continue
            c0 = t0
            for ei, e in enumerate(code):
                if ei:
                    t0 += u * j()
                d = (u if e == "." else dash * u) * j()
                marks.append((t0, t0 + d, len(chars)))
                t0 += d
            chars.append((c0, t0, ch))
    return marks, chars, t0


def example(r: random.Random, n: int):
    """One window: (key-down array at FS, label indices, params)."""
    wpm = float(np.exp(r.uniform(np.log(10), np.log(45))))
    jitter = r.choice([0.0, 0.0, r.uniform(0, 0.08), r.uniform(0.05, 0.18)])
    dash = r.uniform(2.6, 3.8) if r.random() < 0.5 else 3.0
    fw = r.uniform(0.6, 1.0) * wpm if r.random() < 0.15 else wpm
    line = text(r)
    while True:
        marks, chars, total = keying(line, r, wpm, jitter, dash, fw)
        if total >= WINDOW + 1 or r.random() < 0.2:
            break
        # the next phrase after a pause (most of the time) or straight on
        line = line + (" | " if r.random() < 0.7 else " ") + text(r)
    # the window: often starting before the text or ending after it (silence)
    w0 = r.uniform(-3.0, max(-2.9, total - WINDOW + 3.0))
    w1 = w0 + WINDOW
    # characters wholly inside; marks of cut characters are dropped
    ok = [a >= w0 and b <= w1 for a, b, _ in chars]
    inside = [c for c, o in zip(chars, ok) if o]
    key = np.zeros(n, np.float32)
    for m0, m1, ci in marks:
        if ok[ci]:
            i0, i1 = int(round((m0 - w0) * FS)), int(round((m1 - w0) * FS))
            key[max(i0, 0):max(i1, i0 + 1)] = 1.0
    label = []
    for a, b, c in inside:
        if c == " " and (not label or label[-1] == INDEX[" "]):
            continue
        label.append(INDEX[c])
    while label and label[-1] == INDEX[" "]:
        label.pop()
    return key, label, wpm


QRM = 3  # interferers at most


PATHS = 16


def fading(B: int, M: int, t, U, g, fl):
    """(B, M, n) complex gains, mean power 1: per signal, slow QSB (Jakes,
    0.03-0.6 Hz Doppler) on 40 % or none; where `fl` (B, 1), auroral
    flutter instead: a Gaussian Doppler spectrum (ITU-R F.1487) of spread
    (2 sigma) 3-30 Hz, log-uniform, half of them two modes (paths) shifted
    apart by up to 10 Hz. Sum of PATHS sinusoids at random frequencies."""
    dev = t.device
    two_pi = 2 * torch.pi
    fd = U(B, M, 1, a=0.03, b=0.6)
    jakes = fd * torch.cos(U(B, M, PATHS, b=two_pi))                     # Hz
    spread = torch.exp(U(B, M, 1, a=float(np.log(3.0)), b=float(np.log(30.0))))
    shift = U(B, M, 1, b=5.0) * (U(B, M, 1) < 0.5) * torch.where(torch.arange(PATHS, device=dev) % 2 == 0, 1.0, -1.0)
    gauss = shift + spread / 2 * torch.randn(B, M, PATHS, device=dev, generator=g)
    f = torch.where(fl[:, :, None], gauss, jakes)
    p = U(B, M, PATHS, b=two_pi)
    gain = torch.zeros(B, M, t.shape[0], dtype=torch.complex64, device=dev)
    for k in range(PATHS):
        gain = gain + torch.exp(1j * (two_pi * f[:, :, k, None] * t[None, None, :] + p[:, :, k, None]))
    gain = gain / PATHS ** 0.5
    on = fl | (U(B, M) < 0.4)
    return torch.where(on[:, :, None], gain, torch.ones_like(gain))


def batch(B: int, r: random.Random, device, n: int = int(WINDOW * FS), snr=(-12.0, 20.0), pileup=0.3, flutter=0.15):
    """(B, n) complex channels and labels. `pileup`: the share of examples
    with 1-3 close neighbours at similar level (else one signal 25-180 Hz
    away, mostly weaker, on 30 %). `flutter`: the share with auroral
    flutter (the neighbours too)."""
    keys, labels, qrm, mode = [], [], [], []
    for _ in range(B):
        k, lab, _ = example(r, n)
        p = r.random() < pileup
        if r.random() < (0.15 if p else 0.05):
            # noise (and maybe the others) only: nothing to read
            k, lab = np.zeros(n, np.float32), []
        keys.append(k)
        labels.append(lab)
        m = r.randint(1, QRM) if p else int(r.random() < 0.3)
        qrm.append(np.stack([example(r, n)[0] if i < m else np.zeros(n, np.float32) for i in range(QRM)]))
        mode.append(p)
    K = torch.tensor(np.stack(keys), device=device)
    Q = torch.tensor(np.stack(qrm), device=device)             # (B, QRM, n)
    P = torch.tensor(mode, device=device)[:, None]             # (B, 1) pile-up
    t = torch.arange(n, device=device) / FS
    # edges: raised cosine, 2-8 ms
    rise = r.uniform(1.0, 4.0)
    m = max(1, int(rise))
    win = torch.hann_window(2 * m + 3, periodic=False, device=device)
    win = (win / win.sum())[None, None, :]
    K = torch.nn.functional.conv1d(K[:, None, :], win, padding=m + 1)[:, 0, :]
    Q = torch.nn.functional.conv1d(Q.reshape(B * QRM, 1, n), win, padding=m + 1).reshape(B, QRM, n)
    g = torch.Generator(device=device).manual_seed(r.randrange(1 << 30))
    U = lambda *s, a=0.0, b=1.0: a + (b - a) * torch.rand(*s, device=device, generator=g)  # noqa: E731
    two_pi = 2 * torch.pi
    # carrier: residual offset after centring (pile-ups: the skimmer
    # centres on the signal's own carrier, ±4 Hz), slow drift
    res = torch.where(P, 4.0, 10.0)
    f0 = U(B, 1, a=-1.0, b=1.0) * res + U(B, 1, a=-1.0, b=1.0) * t[None, :] / WINDOW
    ph = two_pi * torch.cumsum(f0 / FS, dim=1) + U(B, 1, b=two_pi)
    sig = K * torch.exp(1j * ph)
    # fading: slow QSB (Jakes, 0.03-0.6 Hz) on ~40 %; auroral flutter on
    # `flutter` of them instead (below); the others fade on their own
    fl = U(B, 1) < flutter                                   # (B, 1)
    sig = sig * fading(B, 1, t, U, g, fl)[:, 0, :]
    # the others: 25-180 Hz away at -20..+6 dB; pile-ups 12-120 Hz at -6..+10 dB
    sign = torch.where(U(B, QRM) < 0.5, -1.0, 1.0)
    off = torch.where(P, U(B, QRM, a=12.0, b=120.0), U(B, QRM, a=25.0, b=180.0)) * sign
    lvl = 10 ** (torch.where(P, U(B, QRM, a=-6.0, b=10.0), U(B, QRM, a=-20.0, b=6.0)) / 20)
    ph_q = two_pi * off[:, :, None] * t[None, None, :] + U(B, QRM, 1, b=two_pi)
    sig = sig + (lvl[:, :, None] * Q * fading(B, QRM, t, U, g, fl) * torch.exp(1j * ph_q)).sum(1)
    # white noise: SNR in 2500 Hz for unit mark power
    s = U(B, 1, a=snr[0], b=snr[1])
    n0 = 10 ** (-s / 10) / 2500.0
    sd = torch.sqrt(n0 * FS / 2)
    noise = sd * torch.complex(torch.randn(B, n, device=device, generator=g), torch.randn(B, n, device=device, generator=g))
    # impulse noise on ~20 %: sparse clicks
    clicks = (U(B, n) < 0.002) * (U(B, 1) < 0.2) * U(B, n, a=0.0, b=20.0) * sd
    x = sig + noise + clicks * torch.exp(1j * U(B, n, b=two_pi))
    # level: anywhere (the features normalise)
    x = x * 10 ** (U(B, 1, a=-3.0, b=3.0))
    return x.to(torch.complex64), labels
