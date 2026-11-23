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


class Net(nn.Module):
    def __init__(self, hidden: int = 96):
        super().__init__()
        self.conv = nn.Conv1d(BINS, hidden, 3)
        self.gru = nn.GRU(hidden, hidden, 2, batch_first=True)
        self.out = nn.Linear(hidden, BINS)

    def forward(self, f: torch.Tensor) -> torch.Tensor:
        """Features (B, T, BINS) → gains (B, T, BINS) in 0..1."""
        h = F.relu(self.conv(F.pad(f.transpose(1, 2), (2, 0)))).transpose(1, 2)
        h, _ = self.gru(h)
        return torch.sigmoid(self.out(h))


def enhance(net: Net, x: torch.Tensor, floor: float = 0.0) -> torch.Tensor:
    X = stft(x)
    g = net(features(X))
    g = floor + (1 - floor) * g
    return istft(X * g, x.shape[-1])
