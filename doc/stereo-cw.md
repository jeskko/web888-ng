# Pseudo-stereo CW: draft

Status: **draft** (2026-10-28), not started. The idea comes from SM0VPO's
analogue circuit ("Enhance weak QRP CW with stereo"): a buffer feeds a
low-pass to the left ear and a high-pass to the right, crossing near
800 Hz. A signal at the pitch sits in the middle of the head, and QRM
above or below it moves off to the sides. The ear separates sources by
direction much better than by pitch alone, so a weak signal next to a
strong one gets easier to copy.

In software we can do better than two RC filters. Each audio frequency
gets its own position, a smooth ramp from left to right, so every
signal in the passband has a place, not just "low" or "high".

## 1. What the listener gets

* A **Stereo** toggle in the audio controls, for CW mode (§6 covers the
  other modes). Per viewer, remembered in `localStorage` like the spots
  panel. Headphones are needed; on speakers it just sounds wider.
* The image follows the waterfall. A signal at the dial (zero beat at
  the pitch) is in the centre. One below it is to the left, one above it
  to the right, as on the screen. Tuning a signal into the centre of the
  head is then also a way to zero-beat it.
* A **width** setting: how far from the pitch a signal has to be to sit
  fully at one side. Its default is the passband's half-width, so the
  passband's edges land at the two ears (±250 Hz for the default CW
  filter).

## 2. Where it runs: in the browser

The server sends mono 12 kS/s audio as now, and the AudioWorklet makes
it stereo. That keeps it off the A9 (no server CPU per user), with no
change to the audio bandwidth or the protocol. Listening along works
unchanged. The worklet already has a two-channel output; today it copies
the mono signal into both.

```mermaid
flowchart LR
  ws["WebSocket audio<br/>mono 12 kS/s"] --> sp["stereo FIR pair<br/>12 kS/s, L and R"]
  sp --> rs["resampler<br/>2 channels, one clock"]
  rs --> out["AudioWorklet output<br/>L, R at the card rate"]
```

The filters run at 12 kS/s before the resampler, not at the card's
48 kHz after it: four times fewer taps and samples. The resampler then
carries two channels on the same read position and trim, so L and R
can't drift apart. Two independent `Resampler`s fed the same input would
stay in step too, but one with a channel count is cleaner.

## 3. The filters

The pan position of audio frequency `f` (Hz) is

    p(f) = clamp((f - pitch) / width, -1, 1)        -1 = left, +1 = right

with a constant-power pan law, so a signal is equally loud wherever it
sits:

    θ(f) = (p(f) + 1) · π/4,   L(f) = cos θ(f),   R(f) = sin θ(f)

A pair of linear-phase FIR filters realises `L(f)` and `R(f)`. They are
designed by frequency sampling: sample the two gain curves, inverse-FFT,
and apply a window (Kaiser). They are rebuilt when the pitch, the width
or the passband changes, which takes well under a millisecond.

* **Length: 255 taps** at 12 kS/s. The resolution is then about 50 Hz,
  enough to resolve a ±250 Hz ramp. The latency is 127 samples, 10.6 ms.
  The cost is 2 × 255 multiply-adds per sample, about 6 M/s: fine in
  JavaScript, even on a phone. If not, use FFT convolution.
* Both filters are linear-phase with the same delay, so L and R stay
  time-aligned. The image is then only level differences, like the
  analogue circuit.
* `p` is clamped: outside ±width the sound stays at the ear, and outside
  the passband there's nothing left anyway (the server has filtered it).
* Possibly a `max pan` below 1 (say 0.85, so the far ear still hears a
  little). Hard-panned sound gets tiring over a long session.

### 3.1 Option: time differences too

Below about 1.5 kHz, where CW lives, the ear judges direction mostly by
the **time difference** between the ears, not the level difference. A
level-only image is weaker than a real one. The same frequency-sampling
design can add a per-frequency delay: `H_L(f) = L(f)·e^{-j2πf(D - τ(f))}`
and `H_R(f) = R(f)·e^{-j2πf(D + τ(f))}`, with `τ(f) = p(f)·τmax` and
τmax ≈ 0.3–0.5 ms (a real head is up to about 0.65 ms). Above roughly
1 kHz, a tone's phase difference gets ambiguous, so τmax stays small.
This is worth a listening test before deciding. It costs nothing extra
at run time, since only the filter design changes.

## 4. Modes and sidebands

The demodulator puts CW on the upper side: audio frequency = RF offset
from the dial + pitch. So a higher audio pitch means a higher RF, and
left/right as above matches the waterfall. If a CW-reverse mode (LSB
side) is added later, flip the sign of `p` so the image still matches
the screen.

## 5. What it doesn't fix

* **AGC.** A strong signal nearby still pumps the AGC and pushes the
  weak one down. The stereo image helps separate it, but doesn't bring
  it back. A narrow filter or a slower AGC is still the tool for that.
* **Distortion.** SM0VPO warns that harmonics land in other places in
  the image. Our audio chain is linear, but clipping (gain too high, AGC
  off) would scatter the same way.

## 6. Later, maybe

* **SSB and AM:** the same thing, centred on the passband's middle
  (~1500 Hz for USB). For voice it's mostly a novelty, but it does help
  on a crowded digital sub-band.
* A **centre marker** on the waterfall's passband overlay, showing where
  "straight ahead" is.

## 7. Work, when it's picked up

1. `ui/src/audio/stereo.ts`: the filter design and the FIR pair, pure
   and unit-tested:
   * at `pitch`, L = R;
   * at `pitch ± width`, one channel is close to zero;
   * `|L|² + |R|²` is about 1 across the passband;
   * the impulse responses are symmetric (linear phase).
2. A `Resampler` with a channel count, and its tests.
3. The worklet takes `{stereo: {pitch, width, lo, hi}}` (or null) and
   runs the pair before the resampler.
4. The UI: a toggle and width next to the pitch, in CW mode, remembered
   per viewer.
5. A listening test with real QRM, level only and level plus time (§3.1),
   to choose the defaults.
