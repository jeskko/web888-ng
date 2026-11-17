// SPDX-License-Identifier: GPL-3.0-or-later
// Audio recording (src/audiorec.ts): the receiver's audio saved as a WAV
// in this browser. A list to pick the longest length and whether to leave
// out silence; while recording the button shows the time and stops (and
// saves) on a click.

import { useEffect, useRef, useState } from "preact/hooks";
import { audioRec, audioRecDone, audioRecSkip, setAudioRecSkip, st, startAudioRec, stopAudioRec } from "../state.ts";

const LENGTHS: [number, string][] = [
  [60, "1 min"],
  [300, "5 min"],
  [900, "15 min"],
  [3600, "1 h"],
];

const mmss = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

export function AudioRecCtl() {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("pointerdown", away);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", away);
      document.removeEventListener("keydown", key);
    };
  }, [open]);
  if (!st.value) return null;
  const r = audioRec.value;
  if (r) {
    const skipped = r.skippedS >= 1 ? `, ${mmss(r.skippedS)} of silence left out` : "";
    return (
      <div class="group iqrec-ctl">
        <button class="iqrec on arec" onClick={() => stopAudioRec()} title={`Recording the audio${skipped}: click to stop and save`}>
          ● {mmss(r.seconds)}
        </button>
        <button class="iqrec-x" onClick={() => stopAudioRec(false)} title="Stop without saving">
          ✕
        </button>
      </div>
    );
  }
  return (
    <div class="group iqrec-ctl" ref={box}>
      <button class={`iqrec${open ? " open" : ""}`} onClick={() => setOpen(!open)} title="Record the audio to a WAV file">
        Rec ▾
      </button>
      {open && (
        <div class="iqrec-pop">
          <p class="dim">
            The audio as it comes from the receiver (its AGC, not this page's volume), 16-bit WAV at the audio rate, collected in this
            browser (~1.4 MB a minute at 12 kS/s). Changing the audio rate or to a stereo mode ends the recording.
          </p>
          <div class="iqrec-row">
            <label title="Leave out the time the audio is silent (squelch closed): a long watch keeps only what was heard">
              <input type="checkbox" checked={audioRecSkip.value} onChange={(e) => setAudioRecSkip(e.currentTarget.checked)} /> Skip silence (with squelch)
            </label>
          </div>
          <div class="iqrec-row">
            {LENGTHS.map(([s, label]) => (
              <button
                data-arec={s}
                onClick={() => {
                  setOpen(false);
                  startAudioRec(s);
                }}
              >
                ● {label}
              </button>
            ))}
          </div>
          {audioRecDone.value && <p class="dim">Last: {audioRecDone.value}</p>}
        </div>
      )}
    </div>
  );
}
