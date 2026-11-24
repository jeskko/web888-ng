# SPDX-License-Identifier: GPL-3.0-or-later
"""The SSB voice noise reduction's STFT, features and model (the Rust
inference must match these exactly).

Audio at 12 kS/s. STFT: 256 points (21 ms), hop 128, a square-root
periodic Hann window for analysis and synthesis (overlap-add gives the
input back). Features per frame: log10 of the 129 bins' power, minus a
running level (an exponential mean, 2 s, of the frame's mean log power;
it starts at the first frame), divided by 2 and clamped to [-6, 6].

Model: a causal convolution over 3 frames to H channels, two GRU layers
of H, a linear layer to 129 gains (sigmoid). Mask only: the output is
the input's spectrum times gains in 0..1, so it can take sound away but
not add it. At run time a floor (the strength) keeps some of what it
takes: g' = floor + (1 - floor) g.
"""
import math

import torch
import torch.nn as nn
import torch.nn.functional as F

FS = 12000.0
NFFT = 256
HOP = 128
BINS = NFFT // 2 + 1
LEVEL_S = 2.0
ALPHA = 1 - math.exp(-HOP / FS / LEVEL_S)     # the running level's step per frame


def window(device=None) -> torch.Tensor:
    return torch.sqrt(torch.hann_window(NFFT, periodic=True, device=device))


def stft(x: torch.Tensor) -> torch.Tensor:
    """(B, n) audio, n a multiple of HOP → (B, n / HOP + 1, BINS)
    complex. Frame t covers samples [(t - 1) HOP, (t + 1) HOP), zeros
    outside: causal, each frame made when its last hop arrives."""
    x = F.pad(x, (NFFT - HOP, HOP))
    fr = x.unfold(-1, NFFT, HOP) * window(x.device)
    return torch.fft.rfft(fr, dim=-1)


def istft(X: torch.Tensor, n: int) -> torch.Tensor:
    """Inverse of stft() (B, T, BINS) → (B, n): hop k of the output is
    frame k's second half plus frame k + 1's first half (overlap-add;
    the windows' squares add up to 1). Latency: one hop after the
    frame's end, as in the page."""
    fr = torch.fft.irfft(X, NFFT, dim=-1) * window(X.device)
    y = fr[:, :-1, HOP:] + fr[:, 1:, :HOP]
    return y.reshape(y.shape[0], -1)[:, :n]


def level(lp: torch.Tensor) -> torch.Tensor:
    """The running level (B, T) of the frames' mean log power (B, T)."""
    out = torch.empty_like(lp)
    m = lp[:, 0]
    for t in range(lp.shape[1]):
        m = m + ALPHA * (lp[:, t] - m)
        out[:, t] = m
    return out


def features(X: torch.Tensor) -> torch.Tensor:
    """(B, T, BINS) complex → (B, T, BINS)."""
    lp = torch.log10(X.real ** 2 + X.imag ** 2 + 1e-10)
    lev = level(lp.mean(-1))
    return torch.clamp((lp - lev[:, :, None]) / 2, -6, 6)


PITCH_N = 512                                # samples in the pitch window (43 ms)
PITCH_LAGS = sorted({int(round(30 * (200 / 30) ** (i / 23))) for i in range(24)})
NBANDS = 32


def pitch_features(x: torch.Tensor) -> torch.Tensor:
    """(B, n) audio → (B, T, len(PITCH_LAGS)): per frame, the normalised
    autocorrelation of the last PITCH_N samples (Hann window) at lags
    of 30-200 samples (400-60 Hz), times 2. Frame t's window ends where
    its STFT frame ends, at (t + 1) HOP."""
    x = F.pad(x, (PITCH_N - HOP, HOP))
    fr = x.unfold(-1, PITCH_N, HOP) * torch.hann_window(PITCH_N, periodic=True, device=x.device)
    P = torch.fft.rfft(fr, 2 * PITCH_N, dim=-1)
    r = torch.fft.irfft(P.real ** 2 + P.imag ** 2, 2 * PITCH_N, dim=-1)
    lags = torch.tensor(PITCH_LAGS, device=x.device)
    return 2 * r[..., lags] / (r[..., :1] + 1e-9)


def band_matrix() -> torch.Tensor:
    """(BINS, NBANDS): a bin's gain from the bands' (linear between the
    band centres, mel-spaced over 0-6 kHz; flat beyond the outer ones)."""
    mel = lambda f: 2595 * math.log10(1 + f / 700)        # noqa: E731
    imel = lambda m: 700 * (10 ** (m / 2595) - 1)          # noqa: E731
    c = [imel(mel(60) + (mel(5800) - mel(60)) * i / (NBANDS - 1)) for i in range(NBANDS)]
    m = torch.zeros(BINS, NBANDS)
    for k in range(BINS):
        f = k * FS / NFFT
        if f <= c[0]:
            m[k, 0] = 1
        elif f >= c[-1]:
            m[k, -1] = 1
        else:
            j = max(i for i in range(NBANDS - 1) if c[i] <= f)
            w = (f - c[j]) / (c[j + 1] - c[j])
            m[k, j], m[k, j + 1] = 1 - w, w
    return m


class Net(nn.Module):
    """hidden: GRU size; bands: gains on NBANDS smooth bands (else per
    bin); pitch: the pitch features as input too."""

    def __init__(self, hidden: int = 96, bands: bool = False, pitch: bool = False):
        super().__init__()
        self.bands, self.pitch = bands, pitch
        nin = BINS + (len(PITCH_LAGS) if pitch else 0)
        self.conv = nn.Conv1d(nin, hidden, 3)
        self.gru = nn.GRU(hidden, hidden, 2, batch_first=True)
        self.out = nn.Linear(hidden, NBANDS if bands else BINS)
        if bands:
            self.register_buffer("band", band_matrix().T.contiguous())   # (NBANDS, BINS)

    def forward(self, f: torch.Tensor) -> torch.Tensor:
        """Features (B, T, nin) → gains (B, T, BINS) in 0..1."""
        h = F.relu(self.conv(F.pad(f.transpose(1, 2), (2, 0)))).transpose(1, 2)
        h, _ = self.gru(h)
        g = torch.sigmoid(self.out(h))
        return g @ self.band if self.bands else g

    def gains(self, x: torch.Tensor, X: torch.Tensor) -> torch.Tensor:
        """Gains for audio x (B, n) and its stft X."""
        f = features(X)
        if self.pitch:
            f = torch.cat([f, pitch_features(x)], dim=-1)
        return self(f)


def load(ck: dict, device) -> Net:
    """The model of a checkpoint (its architecture from the training's
    arguments)."""
    a = ck.get("args", {})
    net = Net(ck.get("hidden", 96), a.get("bands", False), a.get("pitch", False)).to(device)
    net.load_state_dict(ck["model"])
    return net.eval()


def enhance(net: Net, x: torch.Tensor, floor: float = 0.0) -> torch.Tensor:
    X = stft(x)
    g = net.gains(x, X)
    g = floor + (1 - floor) * g
    return istft(X * g, x.shape[-1])
