# SPDX-License-Identifier: GPL-3.0-or-later
"""The neural CW decoder's alphabet, features and model, shared by training
and export. The features and model must match crates/cw/src/net.rs exactly.

Input: a CW channel as the skimmer makes it (complex baseband at 500 S/s,
the carrier near 0 Hz). Features: a 16-point STFT (periodic Hann) every 2
samples (4 ms); log power of the 5 bins around 0 Hz (-62.5..62.5 Hz),
minus the 20th percentile of the centre bin's log power over the window,
divided by 4 and clamped to [-2, 8]. Model: three 1-D convolutions (the
last two stride 2: 62.5 frames/s), a 2-layer bidirectional GRU, a linear
layer to the classes (0 = CTC blank).
"""
import torch
import torch.nn as nn
import torch.nn.functional as F

FS = 500.0
NFFT = 16
HOP = 2
BINS = (-2, -1, 0, 1, 2)
NFEAT = len(BINS)
STRIDE = 4                     # model frames per feature frame

PROSIGNS = ["<SK>", "<KN>", "<AS>", "<BK>", "<HH>"]
CHARS = list("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,?/=+-'():;\"@!&") + PROSIGNS
CLASSES = ["_", " "] + CHARS   # 0: blank
INDEX = {c: i for i, c in enumerate(CLASSES)}

MORSE = {
    "A": ".-", "B": "-...", "C": "-.-.", "D": "-..", "E": ".", "F": "..-.", "G": "--.", "H": "....",
    "I": "..", "J": ".---", "K": "-.-", "L": ".-..", "M": "--", "N": "-.", "O": "---", "P": ".--.",
    "Q": "--.-", "R": ".-.", "S": "...", "T": "-", "U": "..-", "V": "...-", "W": ".--", "X": "-..-",
    "Y": "-.--", "Z": "--..", "0": "-----", "1": ".----", "2": "..---", "3": "...--", "4": "....-",
    "5": ".....", "6": "-....", "7": "--...", "8": "---..", "9": "----.", ".": ".-.-.-", ",": "--..--",
    "?": "..--..", "/": "-..-.", "=": "-...-", "+": ".-.-.", "-": "-....-", "'": ".----.",
    "(": "-.--.", ")": "-.--.-", ":": "---...", ";": "-.-.-.", '"': ".-..-.", "@": ".--.-.",
    "!": "-.-.--", "&": ".-...", "<SK>": "...-.-", "<KN>": "-.--.", "<AS>": ".-...", "<BK>": "-...-.-",
    "<HH>": "........",
}


def features(x: torch.Tensor) -> torch.Tensor:
    """(B, N) complex -> (B, NFEAT, T) with T = (N - NFFT) // HOP + 1."""
    n = torch.arange(NFFT, device=x.device)
    win = 0.5 - 0.5 * torch.cos(2 * torch.pi * n / NFFT)
    fr = x.unfold(-1, NFFT, HOP) * win                          # (B, T, NFFT)
    k = torch.tensor(BINS, device=x.device)
    basis = torch.exp(-2j * torch.pi * k[:, None] * n[None, :] / NFFT).to(fr.dtype)   # (5, NFFT)
    X = fr @ basis.T                                            # (B, T, 5)
    lp = torch.log(X.real ** 2 + X.imag ** 2 + 1e-12)
    floor = torch.quantile(lp[:, :, 2], 0.2, dim=1, interpolation="linear")
    f = ((lp - floor[:, None, None]) / 4.0).clamp(-2.0, 8.0)
    return f.transpose(1, 2).float()


class CwNet(nn.Module):
    def __init__(self, ch=32, ch2=48, hid=48, layers=2):
        super().__init__()
        self.c1 = nn.Conv1d(NFEAT, ch, 5, padding=2)
        self.c2 = nn.Conv1d(ch, ch2, 5, stride=2, padding=2)
        self.c3 = nn.Conv1d(ch2, ch2, 5, stride=2, padding=2)
        # batch norm after each convolution (folded into it at export)
        self.n1, self.n2, self.n3 = nn.BatchNorm1d(ch), nn.BatchNorm1d(ch2), nn.BatchNorm1d(ch2)
        self.gru = nn.GRU(ch2, hid, num_layers=layers, bidirectional=True, batch_first=True)
        self.fc = nn.Linear(2 * hid, len(CLASSES))

    def forward(self, f):                     # (B, NFEAT, T) -> (B, T/4, C) log-probs
        h = F.relu(self.n1(self.c1(f)))
        h = F.relu(self.n2(self.c2(h)))
        h = F.relu(self.n3(self.c3(h)))
        h, _ = self.gru(h.transpose(1, 2))
        return F.log_softmax(self.fc(h), dim=-1)


def fold_bn(sd: dict) -> dict:
    """The state dict with each batch norm folded into its convolution
    (inference: y = g (conv(x) - mean) / sqrt(var + eps) + b)."""
    out = {k: v for k, v in sd.items() if not k.startswith(("n1.", "n2.", "n3."))}
    for c, n in (("c1", "n1"), ("c2", "n2"), ("c3", "n3")):
        if f"{n}.weight" not in sd:
            continue
        s = sd[f"{n}.weight"] / torch.sqrt(sd[f"{n}.running_var"] + 1e-5)
        out[f"{c}.weight"] = sd[f"{c}.weight"] * s[:, None, None]
        out[f"{c}.bias"] = (sd[f"{c}.bias"] - sd[f"{n}.running_mean"]) * s + sd[f"{n}.bias"]
    return out


def out_len(t: int) -> int:
    for _ in range(2):
        t = (t + 2 * 2 - 5) // 2 + 1
    return t


def greedy(lp: torch.Tensor) -> list[str]:
    """(B, T, C) -> texts."""
    best = lp.argmax(-1).cpu().numpy()
    out = []
    for row in best:
        s, prev = [], 0
        for c in row:
            if c != prev and c != 0:
                s.append(CLASSES[c])
            prev = c
        out.append("".join(s).strip())
    return out
