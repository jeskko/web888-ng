# SPDX-License-Identifier: GPL-3.0-or-later
"""A frozen speech encoder for a perceptual (feature) loss: wav2vec 2.0
Base (Baevski et al. 2020), written out here so that only its weights are
downloaded (no outside code runs), loaded with weights_only.

Weights: torchaudio's copy of the fairseq release, MIT licence,
pre-trained (self-supervised, not fine-tuned) on LibriSpeech's 960 h
(CC BY 4.0):

    https://download.pytorch.org/torchaudio/models/wav2vec2_fairseq_base_ls960.pth
    sha256 1275f890a623b31b9915295dc01d5049c93dc6de8813406c16dfab9b6bd36056

    NR_SSL=/data/iq/nr/ssl/wav2vec2_fairseq_base_ls960.pth

The architecture (the post-LN "base" variant): seven convolutions
(512 channels, kernels 10,3,3,3,3,2,2, strides 5,2,2,2,2,2,2, no bias;
GroupNorm on the first; GELU) → 20 ms frames at 16 kS/s; LayerNorm and
a projection to 768; a grouped convolutional position embedding (kernel
128, 16 groups, weight norm over dim 2); LayerNorm; 12 transformer
layers (12 heads, post-LN, GELU feed-forward 3072).

The loss uses the convolutional features and the first few transformer
layers (the lower layers carry phonetic detail; the upper ones more
context): `features()` returns them as a list.
"""
import os

import torch
import torch.nn as nn
import torch.nn.functional as F

KERNELS = (10, 3, 3, 3, 3, 2, 2)
STRIDES = (5, 2, 2, 2, 2, 2, 2)
SR = 16000


class Layer(nn.Module):
    def __init__(self, d=768, heads=12, ff=3072):
        super().__init__()
        self.heads = heads
        self.attention = nn.Module()
        for n in ("q_proj", "k_proj", "v_proj", "out_proj"):
            setattr(self.attention, n, nn.Linear(d, d))
        self.layer_norm = nn.LayerNorm(d, eps=1e-5)
        self.feed_forward = nn.Module()
        self.feed_forward.intermediate_dense = nn.Linear(d, ff)
        self.feed_forward.output_dense = nn.Linear(ff, d)
        self.final_layer_norm = nn.LayerNorm(d, eps=1e-5)

    def forward(self, x):
        B, T, D = x.shape
        a = self.attention
        h = self.heads
        q = a.q_proj(x).view(B, T, h, D // h).transpose(1, 2)
        k = a.k_proj(x).view(B, T, h, D // h).transpose(1, 2)
        v = a.v_proj(x).view(B, T, h, D // h).transpose(1, 2)
        y = F.scaled_dot_product_attention(q, k, v).transpose(1, 2).reshape(B, T, D)
        x = self.layer_norm(x + a.out_proj(y))
        f = self.feed_forward
        x = self.final_layer_norm(x + f.output_dense(F.gelu(f.intermediate_dense(x))))
        return x


class Wav2Vec2(nn.Module):
    def __init__(self, layers=12):
        super().__init__()
        fe = nn.Module()
        fe.conv_layers = nn.ModuleList()
        cin = 1
        for i, (k, s) in enumerate(zip(KERNELS, STRIDES)):
            m = nn.Module()
            m.conv = nn.Conv1d(cin, 512, k, s, bias=False)
            if i == 0:
                m.layer_norm = nn.GroupNorm(512, 512)
            fe.conv_layers.append(m)
            cin = 512
        self.feature_extractor = fe
        enc = nn.Module()
        enc.feature_projection = nn.Module()
        enc.feature_projection.layer_norm = nn.LayerNorm(512, eps=1e-5)
        enc.feature_projection.projection = nn.Linear(512, 768)
        tr = nn.Module()
        tr.pos_conv_embed = nn.Module()
        tr.pos_conv_embed.conv = nn.Conv1d(768, 768, 128, padding=64, groups=16)
        tr.layer_norm = nn.LayerNorm(768, eps=1e-5)
        tr.layers = nn.ModuleList(Layer() for _ in range(layers))
        enc.transformer = tr
        self.encoder = enc

    def load(self, path):
        sd = torch.load(path, map_location="cpu", weights_only=True)
        # the weight-normed position convolution: weight = g v / |v| (norm over all but dim 2)
        g = sd.pop("encoder.transformer.pos_conv_embed.conv.weight_g")
        v = sd.pop("encoder.transformer.pos_conv_embed.conv.weight_v")
        sd["encoder.transformer.pos_conv_embed.conv.weight"] = g * v / v.norm(dim=(0, 1), keepdim=True)
        n = len(self.encoder.transformer.layers)
        sd = {k: t for k, t in sd.items()
              if not (k.startswith("encoder.transformer.layers.") and int(k.split(".")[3]) >= n)}
        self.load_state_dict(sd, strict=True)
        return self

    def features(self, x):
        """Audio (B, n) at 16 kS/s → [conv features (B, T, 512), then
        each transformer layer's output (B, T, 768)]."""
        h = x[:, None, :]
        for i, m in enumerate(self.feature_extractor.conv_layers):
            h = m.conv(h)
            if i == 0:
                h = m.layer_norm(h)
            h = F.gelu(h)
        h = h.transpose(1, 2)
        out = [h]
        fp = self.encoder.feature_projection
        h = fp.projection(fp.layer_norm(h))
        tr = self.encoder.transformer
        p = tr.pos_conv_embed.conv(h.transpose(1, 2))[..., :-1]   # even kernel: drop the extra frame
        h = tr.layer_norm(h + F.gelu(p).transpose(1, 2))
        for layer in tr.layers:
            h = layer(h)
            out.append(h)
        return out


def resample_12k_16k(x: torch.Tensor) -> torch.Tensor:
    """(B, n) at 12 kS/s → (B, 4n/3) at 16 kS/s, by zero-padding the
    spectrum (differentiable; everything here is under 6 kHz)."""
    n = x.shape[-1] // 3 * 3
    X = torch.fft.rfft(x[..., :n])
    m = n * 4 // 3
    Y = torch.zeros(x.shape[:-1] + (m // 2 + 1,), dtype=X.dtype, device=x.device)
    Y[..., :X.shape[-1]] = X
    return torch.fft.irfft(Y, m) * (m / n)


def load_ssl(layers=4, device="cuda", path=None):
    """The frozen encoder, its first `layers` transformer layers only."""
    m = Wav2Vec2(layers).load(path or os.environ.get("NR_SSL", "/data/iq/nr/ssl/wav2vec2_fairseq_base_ls960.pth"))
    m = m.to(device).eval()
    for p in m.parameters():
        p.requires_grad_(False)
    return m


def feature_loss(ssl, e: torch.Tensor, t: torch.Tensor) -> torch.Tensor:
    """Audio at 12 kS/s, the restored e against the target t (B, n): the
    mean over the conv features and the layers of the L1 distance, each
    layer's divided by the target features' mean magnitude. Both are
    scaled by the target's RMS (to LibriSpeech-like 0.05), so the
    output's level counts."""
    g = 0.05 / (t.pow(2).mean(-1, keepdim=True).sqrt() + 1e-6)
    fe = ssl.features(resample_12k_16k(e * g))
    with torch.no_grad():
        ft = ssl.features(resample_12k_16k(t * g))
    return sum((a - b).abs().mean() / (b.abs().mean() + 1e-6) for a, b in zip(fe, ft)) / len(ft)
