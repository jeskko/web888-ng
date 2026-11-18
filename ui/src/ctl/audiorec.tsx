// SPDX-License-Identifier: GPL-3.0-or-later
// Audio recording (src/audiorec.ts): the receiver's audio saved as a WAV
// in this browser. A list to pick the longest length and whether to leave
// out silence; while recording the button shows the time and stops (and
// saves) on a click.

import { useEffect, useRef, useState } from "preact/hooks";
import { usePopFocus } from "../a11y.ts";
import { N_, t } from "../i18n.ts";
import { audioRec, audioRecDone, audioRecSkip, setAudioRecSkip, st, startAudioRec, stopAudioRec } from "../state.ts";

const LENGTHS: [number, string][] = [
  [60, N_("1 min")],
  [300, N_("5 min")],
  [900, N_("15 min")],
  [3600, N_("1 h")],
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
  const pop = usePopFocus(open);
  if (!st.value) return null;
  const r = audioRec.value;
  if (r) {
    const skipped = r.skippedS >= 1 ? t(", {time} of silence left out", { time: mmss(r.skippedS) }) : "";
    return (
      <div class="group iqrec-ctl">
        <button class="iqrec on arec" onClick={() => stopAudioRec()} title={t("Recording the audio{skipped}: click to stop and save", { skipped })}>
          ● {mmss(r.seconds)}
        </button>
        <button class="iqrec-x" onClick={() => stopAudioRec(false)} title={t("Stop without saving")} aria-label={t("Stop without saving")}>
          ✕
        </button>
      </div>
    );
  }
  return (
    <div class="group iqrec-ctl" ref={box}>
      <button class={`iqrec${open ? " open" : ""}`} aria-expanded={open} onClick={() => setOpen(!open)} title={t("Record the audio to a WAV file")}>
        {/* i18n: Rec = short for Record (audio recording button) */}
        {t("Rec")} ▾
      </button>
      {open && (
        <div class="iqrec-pop" ref={pop}>
          <p class="dim">
            {/* i18n: AGC = automatic gain control; kS/s = thousand samples per second */}
            {t(
              "The audio as it comes from the receiver (its AGC, not this page's volume), 16-bit WAV at the audio rate, collected in this browser (~1.4 MB a minute at 12 kS/s). Changing the audio rate or to a stereo mode ends the recording.",
            )}
          </p>
          <div class="iqrec-row">
            <label title={t("Leave out the time the audio is silent (squelch closed): a long watch keeps only what was heard")}>
              <input type="checkbox" checked={audioRecSkip.value} onChange={(e) => setAudioRecSkip(e.currentTarget.checked)} /> {t("Skip silence (with squelch)")}
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
                ● {t(label)}
              </button>
            ))}
          </div>
          {audioRecDone.value && <p class="dim">{t("Last: {what}", { what: audioRecDone.value })}</p>}
        </div>
      )}
    </div>
  );
}
