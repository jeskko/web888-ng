# SPDX-License-Identifier: GPL-3.0-or-later
"""The signal identifier's features and model (the Rust inference must
match these exactly).

Input: 4.096 s of complex baseband at 6 kS/s, the signal of interest
near 0 Hz. Three views:

* spectrogram (2, 381, 256): log power of a 256-point STFT (periodic
  Hann, 23.4 Hz, 43 ms) every 64 samples, and of a 64-point one (94 Hz,
  11 ms, keying) on the same grid (each bin repeated 4 times); 0 Hz in
  the middle;
* fine spectrum (1, 2048): log of the 4096-point periodogram averaged
  over 11 half-overlapping frames (1.46 Hz), the middle ±1.5 kHz;
* autocorrelation (1, 400): |R(lag)| / R(0) at lags of 1-400 ms (frame
  and line periods: STANAG 4285's 106.7 ms, MIL-110A's probes, SSTV and
  fax lines).

Log powers are relative to their median, / 4, clamped to [-3, 6].
Model: a 2-D CNN on the spectrogram (pooled over time, frequency kept
coarsely: where a signal is matters, the label is the one at the
centre), 1-D CNNs on the other two, one hidden layer.
"""
import torch
import torch.nn as nn
import torch.nn.functional as F

from modes import CLASSES

N = 24576
LAGS = 400


def _stft_logp(x, n, hop):
    w = 0.5 - 0.5 * torch.cos(2 * torch.pi * torch.arange(n, device=x.device) / n)
    fr = x.unfold(-1, n, hop) * w                              # (B, T, n)
    X = torch.fft.fftshift(torch.fft.fft(fr, dim=-1), dim=-1)
    return torch.log(X.real ** 2 + X.imag ** 2 + 1e-20)


def _norm(lp):
    flat = lp.flatten(1)
    med = flat.median(dim=1).values
    shape = (-1,) + (1,) * (lp.dim() - 1)
    return ((lp - med.view(shape)) / 4.0).clamp(-3.0, 6.0)


def features(x: torch.Tensor):
    """(B, N) complex -> spectrogram (B, 2, 381, 256), fine (B, 1, 2048),
    autocorrelation (B, 1, 400)."""
    a = _stft_logp(x, 256, 64)                                 # (B, 381, 256)
    b = _stft_logp(x, 64, 64)                                  # (B, 384, 64)
    b = b[:, 2:2 + a.shape[1]].repeat_interleave(4, dim=-1)    # centres 64 i + 160 vs 64 i + 128
    spec = torch.stack([_norm(a), _norm(b)], 1)
    fr = x.unfold(-1, 4096, 2048)                              # (B, 11, 4096)
    w = 0.5 - 0.5 * torch.cos(2 * torch.pi * torch.arange(4096, device=x.device) / 4096)
    X = torch.fft.fftshift(torch.fft.fft(fr * w, dim=-1), dim=-1)
    p = (X.real ** 2 + X.imag ** 2).mean(1)[:, 1024:3072]
    fine = _norm(torch.log(p + 1e-20))[:, None, :]
    Z = torch.fft.fft(x, 2 * N)
    r = torch.fft.ifft(Z.real ** 2 + Z.imag ** 2)[:, : 6 * LAGS + 1]
    ac = (r.abs()[:, 6::6] / (r[:, :1].real + 1e-20))[:, None, :LAGS]
    return spec.float(), fine.float(), ac.float()


def block2(cin, cout):
    return nn.Sequential(nn.Conv2d(cin, cout, 3, padding=1), nn.BatchNorm2d(cout), nn.ReLU(), nn.MaxPool2d(2))


def block1(cin, cout, k):
    return nn.Sequential(nn.Conv1d(cin, cout, k, padding=k // 2), nn.BatchNorm1d(cout), nn.ReLU(), nn.MaxPool1d(4 if k == 7 else 2))


class SigId(nn.Module):
    def __init__(self, n_classes=len(CLASSES)):
        super().__init__()
        self.spec = nn.Sequential(block2(2, 16), block2(16, 32), block2(32, 48), block2(48, 64),
                                  nn.Conv2d(64, 64, 3, padding=1), nn.BatchNorm2d(64), nn.ReLU())
        self.fine = nn.Sequential(block1(1, 16, 7), block1(16, 32, 7), block1(32, 32, 7))   # 2048 -> 32
        self.ac = nn.Sequential(block1(1, 16, 5), block1(16, 32, 5))                        # 400 -> 100
        self.head = nn.Sequential(nn.Linear(64 * 2 * 16 + 32 * 16 + 32 * 25, 128), nn.ReLU(), nn.Dropout(0.3),
                                  nn.Linear(128, n_classes))

    def forward(self, spec, fine, ac):
        h = self.spec(spec)                                    # (B, 64, 23, 16)
        h = torch.cat([h.amax(2), h.mean(2)], 1).flatten(1)    # over time: (B, 2048)
        g = F.adaptive_avg_pool1d(self.fine(fine), 16).flatten(1)
        a = F.adaptive_avg_pool1d(self.ac(ac), 25).flatten(1)
        return self.head(torch.cat([h, g, a], 1))
