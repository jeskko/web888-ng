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
* A choice of how the position is made (§3): level, phase, or both.
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

## 3. Three ways to place a signal

The pan position of audio frequency `f` (Hz) is the same for all three:

    p(f) = clamp((f - pitch) / width, -1, 1)        -1 = left, +1 = right

What differs is how a position becomes a difference between the ears.

**(1) Level** (ILD, the analogue circuit's way). A constant-power pan
law, so a signal is equally loud wherever it sits:

    θ(f) = (p(f) + 1) · π/4,   L(f) = cos θ(f),   R(f) = sin θ(f)

At the sides one ear gets nearly nothing.

**(2) Phase or time** (IPD/ITD). Both ears get the same level, and only
the phase between them changes with `p`. There are two flavours:

* **Time delay:** `τ(f) = p(f) · τmax` with τmax ≈ 0.3–0.5 ms (a real
  head is up to about 0.65 ms). The phase difference grows with
  frequency (2πfτ). A delay also shifts the keying envelope, so the
  onsets arrive at different times too, which the ear uses as well.
* **Constant phase:** `φ(f) = p(f) · φmax` with φmax up to 90°: the
  classic binaural receiver's trick, a 90° phasing network (I to one
  ear, Q to the other). A signal above zero beat leads in one ear, one
  below leads in the other. The keying envelope isn't shifted.

Below about 1.5 kHz, where CW lives, the ear judges direction mostly by
this phase or time difference, not by level, so (2) may give the stronger
image. It is also easier on the ears: no ear ever goes quiet. But a
phase difference above 180° is ambiguous (with a time delay that starts
at f > 1/(2τmax), about 1 kHz for 0.5 ms), so the range stays small.
On mono speakers, the two ears' signals partly cancel.

**(3) Both:** a gentler level pan (say ±6 dB at the sides instead of
silence) plus a phase or time difference. A real source off to one side
gives both cues, so this is the most natural, and possibly the best,
image.

Note: none of these gives the full **binaural masking level difference**
(BMLD, up to ~15 dB detection gain for a tone in noise at ~500 Hz). That
needs the noise *at the signal's own frequency* to differ between the
ears from the signal, and a linear filter treats the two alike. What the
filters do is put signals at *different* frequencies in different
places. That is the QRM case, and it's what the analogue circuit does.

### 3.1 One filter pair for all three

All three are a pair of FIR filters with complex responses

    H_L(f) = gL(f) · e^{-j(2πfD - ψ(f)/2)}
    H_R(f) = gR(f) · e^{-j(2πfD + ψ(f)/2)}

where `gL`, `gR` are the levels and `ψ(f)` is the phase difference
(2πf·τ(f) or φ(f)), with D the common delay. They are designed by
frequency sampling: sample the responses, inverse-FFT, and apply a
Kaiser window. The run-time cost is the same for every option, so the
choice is a **user setting** (Stereo: off / level / phase / both), and a
listening test picks the default. The filters are rebuilt when the
pitch, the width, the passband or the option changes, in well under a
millisecond.

* **Length: 255 taps** at 12 kS/s. The resolution is then about 50 Hz,
  enough for a ±250 Hz ramp. The latency is D = 127 samples, 10.6 ms.
  The cost is 2 × 255 multiply-adds per sample, about 6 M/s: fine in
  JavaScript, even on a phone. If not, use FFT convolution.
* Level only (ψ = 0): both filters are linear-phase with the same
  delay, so L and R stay time-aligned.
* `p` is clamped: outside ±width the sound stays at the side, and
  outside the passband there's nothing left anyway (the server has
  filtered it).
* Possibly a `max pan` below 1 for (1): hard-panned sound gets tiring
  over a long session.

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
   * at `pitch ± width`, (1) has one channel close to zero, and (2) has
     the full τmax or φmax between the channels at equal levels;
   * `|L|² + |R|²` is about 1 across the passband;
   * for level only, the impulse responses are symmetric (linear phase).
2. A `Resampler` with a channel count, and its tests.
3. The worklet takes `{stereo: {pitch, width, lo, hi}}` (or null) and
   runs the pair before the resampler.
4. The UI: a toggle and width next to the pitch, in CW mode, remembered
   per viewer.
5. A listening test with real QRM, of level, time, constant phase and
   both (§3), to choose the default option, width, τmax and φmax.
