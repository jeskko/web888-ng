# M7 mobile UI

Status: implemented 2026-10-30 (UI only; no server change). Checked in
headless Chromium with an iPhone 14 Pro profile and touch events against a
real Web-888; **not yet on a physical iPhone** (Safari differences:
`:hover` after a tap, the safe areas, no `navigator.vibrate`).

## 1. Why

The desktop layout on a phone (`doc/m2-web-ui.md`) put every control on
the screen at once: about 55 % of an iPhone's height went to buttons and
sliders, the waterfall got about 10 %, and the controls sat at the top and
in the middle, out of reach of the thumb. Tapping a 24 px frequency digit
to type is not practical either.

## 2. What matters with a phone (priorities)

| tier | what | where |
|---|---|---|
| 1, always | waterfall and spectrum, the dial, audio on/off, the signal meter | the view, the top bar, the dock |
| 2, one tap, thumb reach | mode (USB LSB CW AM), step down/up, passband, band, the VFO wheel | the dock (bottom) |
| 3, one more tap | AGC and its speed and gain, NB, NR, squelch, stereo, volume, zoom (−, +, Centre, Full), colour range, callsign, users, share, spots, status | the *More* sheet |
| not on the phone | tooltips, the step dropdown (the wheel's centre replaces it), keyboard shortcuts, the wheel over a digit | desktop only |

Volume is the phone's own buttons (media volume); the slider is under
*More*.

## 3. The layout (`ui/src/ctl/mobile.tsx`)

Used when `isMobile` (`ui/src/mobile.ts`): a window at most 640 px wide, or
a touch device held sideways (`pointer: coarse` and at most 500 px high).
`?ui=mobile` or `?ui=desktop` forces one.

* **Top**: the dial in large type (tap: keypad), mode, passband, band, and
  a slim signal meter (the OVL lamp only while it is lit).
* **View**: spectrum (22 %) and waterfall fill the rest. Tap tunes, one
  finger drags (pan), two fingers pinch to zoom: every 1.5x change of the
  finger distance is one zoom step (x2), around the fingers' middle.
* **Dock** (thumb zone): left, three rows of 44 px keys: modes; step down,
  step up, passband (tap cycles the presets), *Bands*; *Audio* (start, then
  mute) and *More*. Right, the **VFO wheel**.
* **VFO wheel** (150 px): turn it, 24 clicks a turn, one click is one
  step; the first click lands on the step's grid, as the arrow buttons do
  (`wheelTarget`). The centre shows the step; tapping it goes through 0.1,
  0.5, 1, 5, 25, 50 and 100 kHz (kept in the browser). The waterfall
  follows the dial when it leaves the middle 80 % of the view
  (`tuneInView`).
* **Keypad** (own, not the on-screen keyboard): the keyboard shoves the
  page about on iOS, hides the dock and has no use for a band or a "MHz"
  key. Digits and a point; two Go keys show what the entry would mean, as
  kHz (the keyboard's way: 7074 is 7074 kHz) or as MHz. The callsign is the
  only text field and uses the real keyboard (16 px, so iOS doesn't zoom).
* **Bands**: 160 m to 6 m (IARU region 1 edges, `ui/src/bands.ts`). Where
  you were on the band you leave (frequency and mode) is remembered in the
  browser and comes back; the first time it is a usual place (e.g. 7.100
  LSB). Picking a band also moves the waterfall to the dial, and from a
  wide view (under 128 decimation) zooms to about 240 kHz.

## 4. Notes

* The existing controls (`AudioCtl`, `DspCtl`, `ZoomCtl`, ..., and the
  JTTY button and its list, from v0.6.0) are reused in the *More* sheet, so what they do is the same as on the desktop; only
  their sizes change (`.m-app` in `style.css`: 44 px keys, no `:hover`
  styles, `touch-action: manipulation`, safe-area insets).
* Tuning faster than the server answers (the wheel sends a `tune` per
  click): its `state` for an earlier tune arrived after we had tuned on and
  put the dial back. `ui/src/tunesync.ts` remembers the frequencies sent in
  the last two seconds, and a `state` carrying one of them (not the newest)
  is taken for an old answer and doesn't move the dial; a change the
  server makes itself still does. Reproduced with 80 ms added delay on
  the server's replies (the dial went backwards 5 times in one turn,
  0 times after). This also fixes the desktop's digit wheel.
* Pure logic with tests (`ui/test/mobile.test.ts`): wheel clicks and the
  short-way-round angle, keypad entry and its meaning, band lookup, the
  tune sync.
* Not done: inertia on the wheel (a flick), haptics on iOS (Safari has no
  `vibrate`), a landscape layout of its own (the dock stays at the
  bottom), swipe on the dial, an "add to home screen" manifest.

## 5. Open questions

* Is 24 clicks a turn right at 100 Hz steps (a quarter turn is 600 Hz)?
  Needs a thumb on a real phone.
* Should the bands offer also the broadcast bands and time signals?
