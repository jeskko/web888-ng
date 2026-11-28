# SPDX-License-Identifier: GPL-3.0-or-later
"""The generative SSB voice restorer ("restore, may invent"): a
conditional flow from the mask model's output to clean speech, in the
mask model's causal STFT (common.py), StoRM-shaped (the mask output is
where the flow starts, so few steps and few inventions).

Spectra are normalised and compressed: per frame divided by the noisy
input's running level (lev_of, as an amplitude; --norm), then
c = X |X|^(B - 1) with B = 0.5; real and imaginary parts side by side
(2 x 129).

* Conditioning, once a frame: the noisy input's features
  (common.features), the mask output's (its log power against the same
  running level, /2, clamped), the pitch features; a causal convolution
  over 3 frames to H, two GRU layers of H.
* Flow head, once a step: [GRU output, the mask output's spectrum c_E,
  the state x, a time embedding] → MLP (W, W) → velocity (2 x 129).
  With --mag the same on the compressed magnitude only (129), the noisy
  phase kept (the complex flow at 1-2 steps averages phase it can't
  know, and comes out 3-11 dB too quiet, more at low SNR).
* Path: x0 = c_E + SIGMA eps, x1 = c_S (the clean speech), straight:
  x_t = (1 - t) x0 + t x1, the head learns x1 - x0. Restoring: Euler
  from x0 in K steps (1 or 2 in the page).

* Level (--level L): on the 1-step output x0 + v(x0, 0), per band
  (LEVEL_BANDS) and frame where the target has speech, a hinge on being
  more than --level-margin dB under the mask's output, L x (dB / 10).
  Flow matching alone came out 22-37 dB under the mask on real
  receptions (the consonant bands most), though level on synthetic ones.

Training (run7's mask model frozen, data.py's examples with a clean
target):

    NR_SPEECH=speech.i16 NR_SPEECH_VAL=speech-dev.i16 NR_NOISE=noise.i16 nice -n 10 \\
      python3 gen.py --mask run7/last.pt --out gen1
"""
import argparse
import math
import os
import time

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F

from common import BINS, PITCH_LAGS, features, istft, level, load, pitch_features, stft

BETA = 0.5
SIGMA = 0.2
NT = 16                                        # time embedding: sin and cos of 16 frequencies
LEVEL_BANDS = ((150, 500), (500, 1000), (1000, 2000), (2000, 3500))


def lev_of(X: torch.Tensor, norm: str = "log") -> torch.Tensor:
    """(B, T): the noisy input's running level (log10 power). "log": of
    the frames' mean log power over all bins (the mask model's; the empty
    bins outside the receiver's passband pull it down, by 36 dB on
    synthetic batches and 22 dB on real receptions); "mean": of the log
    of their mean power (the passband's level, whatever the stopband)."""
    p = X.real ** 2 + X.imag ** 2
    m = torch.log10(p + 1e-10).mean(-1) if norm == "log" else torch.log10(p.mean(-1) + 1e-10)
    return level(m)


def scale_of(X: torch.Tensor, norm: str = "log") -> torch.Tensor:
    """(B, T, 1): the noisy input's running level as an amplitude."""
    return torch.pow(10.0, lev_of(X, norm) / 2)[:, :, None]


def comp(X: torch.Tensor, s: torch.Tensor) -> torch.Tensor:
    """(B, T, BINS) complex → (B, T, 2 BINS) real, normalised by s."""
    Z = X / s
    Z = Z * (Z.abs().clamp_min(1e-9) ** (BETA - 1))
    return torch.cat([Z.real, Z.imag], -1)


def comp_mag(X: torch.Tensor, s: torch.Tensor) -> torch.Tensor:
    """(B, T, BINS) complex → (B, T, BINS) compressed magnitude, normalised by s."""
    return (X.abs() / s).clamp_min(1e-9) ** BETA


def decomp_mag(c: torch.Tensor, s: torch.Tensor, X: torch.Tensor) -> torch.Tensor:
    """The magnitude back, with the phase of X (the noisy input's)."""
    ph = X / X.abs().clamp_min(1e-9)
    return c.clamp_min(0) ** (1 / BETA) * s * ph


def decomp(c: torch.Tensor, s: torch.Tensor) -> torch.Tensor:
    Z = torch.complex(c[..., :BINS], c[..., BINS:])
    m = Z.abs().clamp_min(1e-9)
    return Z * m ** (1 / BETA - 1) * s


def temb(t: torch.Tensor) -> torch.Tensor:
    """(..., 1) in 0..1 → (..., 2 NT)."""
    f = torch.arange(1, NT + 1, device=t.device, dtype=t.dtype) * math.pi
    return torch.cat([torch.sin(t * f), torch.cos(t * f)], -1)


class Gen(nn.Module):
    """`mag`: the flow on the compressed magnitude only (BINS values a
    frame), the noisy phase kept; else the complex spectrum (2 BINS)."""

    def __init__(self, hidden: int = 256, width: int = 512, mag: bool = False, norm: str = "log"):
        super().__init__()
        self.mag = mag
        self.norm = norm
        d = BINS if mag else 2 * BINS
        nin = 2 * BINS + len(PITCH_LAGS)
        self.conv = nn.Conv1d(nin, hidden, 3)
        self.gru = nn.GRU(hidden, hidden, 2, batch_first=True)
        self.h1 = nn.Linear(hidden + 2 * d + 2 * NT, width)
        self.h2 = nn.Linear(width, width)
        self.h3 = nn.Linear(width, d)

    def comp(self, X, s):
        return comp_mag(X, s) if self.mag else comp(X, s)

    def decomp(self, c, s, X):
        return decomp_mag(c, s, X) if self.mag else decomp(c, s)

    def cond(self, x: torch.Tensor, X: torch.Tensor, E: torch.Tensor) -> torch.Tensor:
        """Audio x (B, n), its stft X and the mask output's E → (B, T, H)."""
        lp = torch.log10(X.real ** 2 + X.imag ** 2 + 1e-10)
        lev = lev_of(X, self.norm)[:, :, None]
        fx = features(X) if self.norm == "log" else torch.clamp((lp - lev) / 2, -6, 6)
        fe = torch.clamp((torch.log10(E.real ** 2 + E.imag ** 2 + 1e-10) - lev) / 2, -6, 6)
        f = torch.cat([fx, fe, pitch_features(x)], -1)
        h = F.relu(self.conv(F.pad(f.transpose(1, 2), (2, 0)))).transpose(1, 2)
        return self.gru(h)[0]

    def velocity(self, h, ce, xt, t):
        z = torch.cat([h, ce, xt, temb(t)], -1)
        z = F.silu(self.h1(z))
        z = F.silu(self.h2(z))
        return self.h3(z)


def mask_out(mask, x: torch.Tensor, X: torch.Tensor) -> torch.Tensor:
    # frozen: no graph; --joint: the mask learns with the restorer
    if not torch.is_grad_enabled() or not any(p.requires_grad for p in mask.parameters()):
        with torch.no_grad():
            return X * mask.gains(x, X)
    return X * mask.gains(x, X)


@torch.no_grad()
def restore(gen: Gen, mask, x: torch.Tensor, steps: int = 2, seed: int = 0, sigma: float = SIGMA) -> torch.Tensor:
    """Audio (B, n) → restored audio (B, n)."""
    X = stft(x)
    E = mask_out(mask, x, X)
    s = scale_of(X, gen.norm)
    ce = gen.comp(E, s)
    h = gen.cond(x, X, E)
    g = torch.Generator(x.device).manual_seed(seed)
    xt = ce + sigma * torch.randn(ce.shape, generator=g, device=x.device)
    for k in range(steps):
        t = torch.full(ce.shape[:-1] + (1,), k / steps, device=x.device)
        xt = xt + gen.velocity(h, ce, xt, t) / steps
    # (the mask output's phase is the noisy input's: its gains are real)
    return istft(gen.decomp(xt, s, E), x.shape[-1])


def band_pow(gen, c: torch.Tensor) -> torch.Tensor:
    """A compressed spectrum (as gen.comp's) → (B, T, len(LEVEL_BANDS))
    power, on the same normalisation."""
    m2 = c ** 2 if gen.mag else c[..., :BINS] ** 2 + c[..., BINS:] ** 2
    p = m2 ** 2                                 # |Z|^(2 B) squared, B = 0.5
    hz = torch.arange(BINS, device=c.device) * (6000.0 / (BINS - 1))
    return torch.stack([p[..., (hz >= a) & (hz < b)].sum(-1) for a, b in LEVEL_BANDS], -1)


def loss_fn(gen, mask, x, y, w_level=0.0, margin=10.0):
    """The flow-matching loss and, with w_level > 0, the level hinge
    (returned apart, for the log)."""
    X, S = stft(x), stft(y)
    E = mask_out(mask, x, X)
    s = scale_of(X, gen.norm)
    ce, cs = gen.comp(E, s), gen.comp(S, s)
    h = gen.cond(x, X, E)
    t = torch.rand(ce.shape[:-1] + (1,), device=x.device)
    x0 = ce + SIGMA * torch.randn_like(ce)
    xt = (1 - t) * x0 + t * cs
    fm = ((gen.velocity(h, ce, xt, t) - (cs - x0)) ** 2).mean()
    if w_level <= 0:
        return fm, torch.zeros((), device=x.device)
    x1 = x0 + gen.velocity(h, ce, x0, torch.zeros_like(t))
    pg, pm, ps = band_pow(gen, x1), band_pow(gen, ce), band_pow(gen, cs)
    # speech: frames within 20 dB of the target's loudest (all bands)
    tot = ps.sum(-1)
    sp = (tot > 0.01 * tot.amax(1, keepdim=True)).float()[..., None]
    short = F.relu(torch.log10(pm + 1e-8) - margin / 10 - torch.log10(pg + 1e-8))
    lv = (short * sp).sum() / (sp.sum() * len(LEVEL_BANDS) + 1)
    return fm + w_level * lv, lv


DIRECT_BANDS = (150, 300, 500, 750, 1000, 1400, 2000, 2700, 3500)
FEAT_ROWS = 16
FEAT_LEN = 24000                               # 2 s


def direct_loss(gen, mask, x, y, w_band=1.0, ssl=None, w_feat=0.0):
    """--direct: the 1-step output x0 + v(x0, 0) (as the page runs it,
    from the same noisy start) trained as a predictor, not a flow:
    * the compressed magnitudes' squared error against the clean target;
    * the band level (8 bands, 150-3500 Hz) in the target's speech
      frames, two-sided, |dB| / 10: the level and the tilt, against the
      clean speech rather than the mask (whose upper bands are mostly
      residual noise);
    * with w_feat > 0, the frozen speech encoder's feature distance
      (sslfeat.py) between the output and the target magnitude, both
      with the noisy phase (the real pairs' phase is useless: mistuning).
    → (loss, [mse, band, feat])."""
    X, S = stft(x), stft(y)
    E = mask_out(mask, x, X)
    s = scale_of(X, gen.norm)
    ce, cs = gen.comp(E, s), gen.comp(S, s)
    h = gen.cond(x, X, E)
    x0 = ce + SIGMA * torch.randn_like(ce)
    out = x0 + gen.velocity(h, ce, x0, torch.zeros(ce.shape[:-1] + (1,), device=x.device))
    mse = ((out - cs) ** 2).mean()
    m2g = out.clamp_min(0) ** 4 if gen.mag else (out[..., :BINS] ** 2 + out[..., BINS:] ** 2) ** 2
    m2s = cs ** 4 if gen.mag else (cs[..., :BINS] ** 2 + cs[..., BINS:] ** 2) ** 2
    hz = torch.arange(BINS, device=x.device) * (6000.0 / (BINS - 1))
    edges = list(zip(DIRECT_BANDS[:-1], DIRECT_BANDS[1:]))
    pg = torch.stack([m2g[..., (hz >= a) & (hz < b)].sum(-1) for a, b in edges], -1)
    ps = torch.stack([m2s[..., (hz >= a) & (hz < b)].sum(-1) for a, b in edges], -1)
    # each example's band energies over its speech frames (frames within
    # 20 dB of its loudest): the level and the tilt, not frame detail
    # (per frame, band levels in the gaps between formants swing by tens
    # of dB on residual noise). Examples whose target is over 30 dB
    # under the input (real receptions' stretches without speech: zero
    # but for int16 rounding) left out.
    tot = ps.sum(-1)
    sp = (tot > 0.01 * tot.amax(1, keepdim=True)).float()[..., None]
    G, T = (pg * sp).sum(1), (ps * sp).sum(1)                 # (B, bands)
    px = (comp_mag(X, s) ** 4).sum(-1)
    ok = (tot.amax(1) > 1e-3 * px.amax(1)).float()[:, None]
    fl = 1e-3 * T.sum(-1, keepdim=True)                       # 30 dB under the speech's total
    db = (torch.log10(G + fl) - torch.log10(T + fl)).abs()
    band = (db * ok).sum() / (ok.sum() * len(edges) + 1e-6)
    feat = torch.zeros((), device=x.device)
    if w_feat > 0 and ssl is not None:
        from sslfeat import feature_loss
        # on FEAT_ROWS examples, a FEAT_LEN crop each (GPU memory: the
        # encoder's first convolution alone is 512 x 3200 values a second)
        n, r = x.shape[-1], FEAT_ROWS
        e, t = istft(gen.decomp(out[:r], s[:r], E[:r]), n), istft(gen.decomp(cs[:r], s[:r], E[:r]), n)
        o = int(torch.randint(0, max(1, n - FEAT_LEN), ()).item())
        with torch.autocast("cuda", torch.bfloat16):
            feat = feature_loss(ssl, e[:, o:o + FEAT_LEN], t[:, o:o + FEAT_LEN]).float()
    return mse + w_band * band + w_feat * feat, torch.stack([mse.detach(), band.detach(), feat.detach()])


def load_gen(ck: dict, device) -> Gen:
    a = ck.get("args", {})
    g = Gen(a.get("hidden", 256), a.get("width", 512), a.get("mag", False), a.get("norm", "log")).to(device)
    g.load_state_dict(ck["model"])
    return g.eval()


@torch.no_grad()
def evaluate(gen, mask, sets, steps=(1, 2)):
    from train import si_sdr
    from common import enhance
    gen.eval()
    was = mask.training
    mask.eval()
    res = {}
    for k, (x, y, _) in sets.items():
        x, y = x.cuda(), y.cuda()
        base = si_sdr(x, y)
        r = [float(np.nanmean((si_sdr(torch.cat([enhance(mask, x[i:i + 64]) for i in range(0, len(x), 64)]), y)
                               - base).cpu().numpy()))]
        for K in steps:
            e = torch.cat([restore(gen, mask, x[i:i + 64], K) for i in range(0, len(x), 64)])
            r.append(float(np.nanmean((si_sdr(e, y) - base).cpu().numpy())))
            if K == steps[0]:
                # its level against the clean speech (SI-SDR doesn't see it)
                lev = float(10 * torch.log10((e ** 2).mean() / (y ** 2).mean()).item())
        res[k] = r + [lev]
    gen.train()
    mask.train(was)
    return res


def main():
    from data import GpuStream, fixed
    from train import BANDS
    ap = argparse.ArgumentParser()
    ap.add_argument("--mask", required=True, help="the mask model's checkpoint (frozen)")
    ap.add_argument("--steps", type=int, default=60000)
    ap.add_argument("--batch", type=int, default=64)
    ap.add_argument("--lr", type=float, default=1e-3)
    ap.add_argument("--hidden", type=int, default=256)
    ap.add_argument("--width", type=int, default=512)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--snr", default="-20,15")
    ap.add_argument("--real-noise", type=float, default=0.5)
    ap.add_argument("--pairs", type=float, default=0.0,
                    help="share of each batch from real Paderborn pairs (pairbank.py, NR_PAIRS)")
    ap.add_argument("--mag", action="store_true", help="the magnitude only, the noisy phase kept")
    ap.add_argument("--norm", choices=["log", "mean"], default="log",
                    help="the running level spectra are normalised by (lev_of): log = the mask model's, "
                         "mean = the passband's (the page's restore.rs reads only log so far)")
    ap.add_argument("--level", type=float, default=0.0, help="the level hinge's weight (0: none)")
    ap.add_argument("--level-margin", type=float, default=10.0, help="dB under the mask's output allowed")
    ap.add_argument("--direct", action="store_true",
                    help="train the 1-step output as a direct predictor (direct_loss), not the flow")
    ap.add_argument("--band", type=float, default=1.0, help="--direct: the band-level term's weight")
    ap.add_argument("--feat", type=float, default=0.0, help="--direct: the speech-encoder feature loss's weight")
    ap.add_argument("--feat-layers", type=int, default=4, help="--direct: transformer layers the feature loss uses")
    ap.add_argument("--joint", type=float, default=0.0,
                    help="train the mask with the restorer at this share of --lr (0: frozen); the tuned mask "
                         "is saved as OUT/mask.pt (the page would carry it for Gen, the plain NN keeps its own)")
    ap.add_argument("--resume")
    ap.add_argument("--out", default="/tmp/gen")
    a = ap.parse_args()
    dev = "cuda"
    os.makedirs(a.out, exist_ok=True)
    mask_ck = torch.load(a.mask, map_location=dev)
    mask = load(mask_ck, dev)
    for p in mask.parameters():
        p.requires_grad_(a.joint > 0)
    # (cuDNN's GRU backward wants training mode; the mask has no dropout)
    mask.train(a.joint > 0)
    gen = Gen(a.hidden, a.width, a.mag, a.norm).to(dev)
    if a.resume:
        gen.load_state_dict(torch.load(a.resume, map_location=dev)["model"])
    print(f"{sum(p.numel() for p in gen.parameters())} weights")
    sets = {k: fixed(128, 1000 + i, b) for i, (k, b) in enumerate(BANDS.items())}
    snr = tuple(float(v) for v in a.snr.split(","))
    dl = torch.utils.data.DataLoader(GpuStream(int(time.time()), a.batch, snr, a.real_noise, None, p_pairs=a.pairs),
                                     batch_size=None, num_workers=a.workers, multiprocessing_context="spawn",
                                     persistent_workers=True, prefetch_factor=2)
    groups = [{"params": list(gen.parameters()), "lr": a.lr, "weight_decay": 1e-4}]
    if a.joint > 0:
        # no weight decay on the mask: it starts from a trained model
        groups.append({"params": list(mask.parameters()), "lr": a.lr * a.joint, "weight_decay": 0.0})
    opt = torch.optim.AdamW(groups)
    sched = torch.optim.lr_scheduler.OneCycleLR(opt, [g["lr"] for g in groups], total_steps=a.steps, pct_start=0.05)
    if a.direct and not a.mag:
        ap.error("--direct needs --mag")
    ssl = None
    if a.direct and a.feat > 0:
        from sslfeat import load_ssl
        ssl = load_ssl(a.feat_layers, dev)
    t0, run, run_lv = time.time(), 0.0, 0.0
    run_parts = torch.zeros(3)
    print("SI-SDR improvement per band: mask / restored in 1 / 2 steps (1 step's level against the clean speech)")
    for step, b in enumerate(dl, 1):
        x, y = (v.to(dev, non_blocking=True) for v in b)
        if a.direct:
            loss, parts = direct_loss(gen, mask, x, y, a.band, ssl, a.feat)
            lv = parts[1]
            run_parts = 0.98 * run_parts + 0.02 * parts.cpu() if step > 1 else parts.cpu()
        else:
            loss, lv = loss_fn(gen, mask, x, y, a.level, a.level_margin)
        opt.zero_grad()
        loss.backward()
        torch.nn.utils.clip_grad_norm_([p for g in groups for p in g["params"]], 1.0)
        opt.step()
        sched.step()
        run = 0.98 * run + 0.02 * loss.item() if step > 1 else loss.item()
        run_lv = 0.98 * run_lv + 0.02 * lv.item() if step > 1 else lv.item()
        if step % 1000 == 0 or step == a.steps:
            r = evaluate(gen, mask, sets)
            extra = (f"mse {run_parts[0]:.4f} band {10 * run_parts[1]:.1f} dB feat {run_parts[2]:.3f} "
                     if a.direct else "")
            print(f"{step:6d} loss {run:.4f} level {10 * run_lv:.1f} dB " + extra + " ".join(f"{k} {'/'.join(f'{v:+.1f}' for v in r[k][:-1])} ({r[k][-1]:+.1f} dB)" for k in r)
                  + f"  {time.time() - t0:.0f} s", flush=True)
            torch.save({"model": gen.state_dict(), "step": step, "val": r, "args": vars(a)}, f"{a.out}/last.pt")
            if a.joint > 0:
                torch.save({**mask_ck, "model": mask.state_dict(), "step": step}, f"{a.out}/mask.pt")
        if step >= a.steps:
            break


if __name__ == "__main__":
    main()
