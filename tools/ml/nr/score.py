# SPDX-License-Identifier: GPL-3.0-or-later
"""Score the noise reduction on real HF speech: the Paderborn database
(arXiv 2106.02472, Zenodo 4247491, CC BY 4.0): LibriSpeech sent in LSB
and received by KiwiSDRs across Europe, 8 kS/s, five utterances a file.

Each reception goes through raw and through the model (whole file, as a
stream would), resampled to 12 kS/s; its utterances are cut out by the
annotations and transcribed by Whisper (local whisper.cpp); the score is
the word error rate against the LibriSpeech text, over all utterances
and by how hard the raw audio is; and STOI (stoi.py) against the clean
source, a predictor of listeners' intelligibility rather than an ASR's.

    python3 score.py /data/nr/paderborn/ham_radio run/best.pt --files 200
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import tempfile
import wave

import numpy as np
import torch
from scipy.signal import resample_poly

from common import Net, enhance
from stoi import estoi, stoi

MODEL = "/usr/share/whisper.cpp-model-small/ggml-small.bin"
MARGIN = 0.3                     # s around each utterance


def read_wav(path):
    with wave.open(path) as w:
        assert w.getsampwidth() == 2 and w.getnchannels() == 1
        return np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32) / 32768, w.getframerate()


def write_wav(path, x, fs):
    x = np.clip(x / max(1e-9, np.abs(x).max()) * 0.9, -1, 1)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(fs)
        w.writeframes((x * 32767).astype(np.int16).tobytes())


def words(t: str) -> list[str]:
    return re.sub(r"[^A-Z0-9' ]", " ", t.upper()).split()


def edits(a: list[str], b: list[str]) -> int:
    d = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        prev, d[0] = d[0], i
        for j, y in enumerate(b, 1):
            prev, d[j] = d[j], min(d[j] + 1, d[j - 1] + 1, prev + (x != y))
    return d[-1]


def lag(clean: np.ndarray, x: np.ndarray, most: int = 800) -> int:
    """The reception's delay against the clean source (samples)."""
    n = min(len(clean), len(x))
    cc = np.fft.irfft(np.fft.rfft(x[:n], 2 * n) * np.conj(np.fft.rfft(clean[:n], 2 * n)))
    return int(np.argmax(np.abs(np.concatenate([cc[-most:], cc[:most + 1]])))) - most


ASR_MODELS = os.environ.get("NR_ASR_MODELS", "/data/iq/nr/asr-models")


def vosk(paths: list[str]) -> list[str]:
    """Vosk (Kaldi), en-us 0.22 lgraph: an older, less noise-robust
    recogniser than Whisper (needs the vosk package)."""
    import vosk as vk
    vk.SetLogLevel(-1)
    m = vk.Model(f"{ASR_MODELS}/vosk/vosk-model-en-us-0.22-lgraph")
    out = []
    for p in paths:
        r = vk.KaldiRecognizer(m, 16000)
        with wave.open(p) as w:
            r.AcceptWaveform(w.readframes(w.getnframes()))
        out.append(json.loads(r.FinalResult()).get("text", ""))
    return out


def parakeet(paths: list[str]) -> list[str]:
    """NVIDIA Parakeet TDT 0.6B v2 (int8, sherpa-onnx): a current
    non-Whisper recogniser (needs the sherpa-onnx package)."""
    import sherpa_onnx
    d = f"{ASR_MODELS}/parakeet/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8"
    rec = sherpa_onnx.OfflineRecognizer.from_transducer(
        encoder=f"{d}/encoder.int8.onnx", decoder=f"{d}/decoder.int8.onnx", joiner=f"{d}/joiner.int8.onnx",
        tokens=f"{d}/tokens.txt", model_type="nemo_transducer", num_threads=8)
    out = []
    for i in range(0, len(paths), 32):
        streams = []
        for p in paths[i:i + 32]:
            x, fs = read_wav(p)
            st = rec.create_stream()
            st.accept_waveform(fs, x)
            streams.append(st)
        rec.decode_streams(streams)
        out += [st.result.text for st in streams]
    return out


def whisper(paths: list[str]) -> list[str]:
    out = []
    for i in range(0, len(paths), 200):
        chunk = paths[i:i + 200]
        cmd = ["whisper-cli", "-m", MODEL, "-l", "en", "-np", "-nt", "-otxt", "-t", "4"]
        for p in chunk:
            cmd += ["-f", p]
        subprocess.run(cmd, check=True, capture_output=True)
        for p in chunk:
            with open(p + ".txt") as f:
                out.append(f.read().strip())
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("db")
    ap.add_argument("ckpt", nargs="?")
    ap.add_argument("--files", type=int, default=200)
    ap.add_argument("--floors", default="0,0.15")
    ap.add_argument("--split", default="eval")
    ap.add_argument("--asr", default="whisper", help="recognisers, comma-separated: whisper, vosk, parakeet")
    ap.add_argument("--stoi-only", action="store_true", help="no Whisper: STOI only (fast)")
    ap.add_argument("--keep", action="store_true", help="keep the audio (else deleted at the end)")
    a = ap.parse_args()
    ann = json.load(open(f"{a.db}/annotations.json"))
    txt = json.load(open(f"{a.db}/transcription.json"))
    files = sorted(os.path.join(d, f) for d, _, fs in os.walk(f"{a.db}/noisy/{a.split}") for f in fs
                   if f.endswith(".wav"))
    rng = np.random.default_rng(1)
    files = [files[i] for i in sorted(rng.choice(len(files), min(a.files, len(files)), replace=False))]
    dev = "cuda" if torch.cuda.is_available() else "cpu"
    net = None
    if a.ckpt:
        c = torch.load(a.ckpt, map_location=dev)
        net = Net(c.get("hidden", 96)).to(dev).eval()
        net.load_state_dict(c["model"])
    conds = ["raw"] + ([f"nn{f}" for f in a.floors.split(",")] if net else [])
    tmp = tempfile.mkdtemp(prefix="nrscore")
    jobs = {c: [] for c in conds}
    refs = []
    st = {c: [] for c in conds}
    es = {c: [] for c in conds}
    for fi, path in enumerate(files):
        k = path.rsplit("__", 1)[1][:-4]
        x8, fs = read_wav(path)
        clean, _ = read_wav(f"{a.db}/clean/{a.split}/clean_{k}.wav")
        d = lag(clean, x8)
        x = resample_poly(x8, 3, 2).astype(np.float32)           # 8 → 12 kS/s
        outs = {"raw": x}
        if net:
            xt = torch.from_numpy(x[: len(x) // 128 * 128])[None].to(dev)
            with torch.no_grad():
                for f in a.floors.split(","):
                    outs[f"nn{f}"] = enhance(net, xt, float(f))[0].cpu().numpy()
        spans = [tuple(map(int, s.split(":"))) for s in ann[k]["activity"][0].split(", ")]
        for u, ((s0, s1), ref) in enumerate(zip(spans, txt[k])):
            i0, i1 = int(max(0, s0 / fs - MARGIN) * 12000), int((s1 / fs + MARGIN) * 12000)
            # STOI against the clean source, at 8 kS/s, the delay taken out
            for c in conds:
                y8 = resample_poly(outs[c], 2, 3)
                j0, j1 = s0 + d, min(s1 + d, len(y8))
                st[c].append(stoi(clean[s0:j1 - d], y8[j0:j1], fs))
                es[c].append(estoi(clean[s0:j1 - d], y8[j0:j1], fs))
            for c in conds if not a.stoi_only else []:
                y = outs[c][i0:i1]
                p = f"{tmp}/{fi}_{u}_{c}.wav"
                write_wav(p, resample_poly(y, 4, 3), 16000)       # Whisper takes 16 kS/s
                jobs[c].append(p)
            refs.append(words(ref))
    if a.stoi_only:
        print(f"{len(files)} receptions, {len(refs)} utterances")
        print(f"{'STOI':16}" + "".join(f"{c:>10}" for c in conds))
        print(f"{'all':16}" + "".join(f"{np.nanmean(st[c]):10.3f}" for c in conds))
        print(f"{'ESTOI all':16}" + "".join(f"{np.nanmean(es[c]):10.3f}" for c in conds))
        shutil.rmtree(tmp)
        return
    print(f"{len(files)} receptions, {len(refs)} utterances, {sum(map(len, refs))} words")
    engines = {"whisper": whisper, "vosk": vosk, "parakeet": parakeet}
    for eng in a.asr.split(","):
        hyp = {c: [words(t) for t in engines[eng](jobs[c])] for c in conds}
        raw_err = np.array([edits(r, h) / max(1, len(r)) for r, h in zip(refs, hyp["raw"])])
        bands = [("all", np.ones(len(refs), bool)), ("raw WER < 30 %", raw_err < 0.3),
                 ("30-80 %", (raw_err >= 0.3) & (raw_err < 0.8)), ("raw WER >= 80 %", raw_err >= 0.8)]
        print(f"\nWER, {eng}")
        print(f"{'':16}" + "".join(f"{c:>10}" for c in conds))
        for name, m in bands:
            row = []
            for c in conds:
                e = sum(edits(r, h) for r, h, ok in zip(refs, hyp[c], m) if ok)
                n = sum(len(r) for r, ok in zip(refs, m) if ok)
                row.append(f"{100 * e / max(1, n):9.1f}%")
            print(f"{name:16}" + "".join(row) + f"   ({int(m.sum())} utterances)")
    print()
    print(f"{'STOI':16}" + "".join(f"{np.nanmean(st[c]):10.3f}" for c in conds))
    print(f"{'ESTOI':16}" + "".join(f"{np.nanmean(es[c]):10.3f}" for c in conds))
    if a.keep:
        print(f"audio in {tmp}")
    else:
        shutil.rmtree(tmp)


if __name__ == "__main__":
    main()
