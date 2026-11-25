// SPDX-License-Identifier: GPL-3.0-or-later
// Audio recording (src/audiorec.ts): the receiver's audio saved as a WAV
// in this browser. A list to pick the longest length and whether to leave
// out silence; while recording the button shows the time and stops (and
// saves) on a click.

import { Pop } from "./pop.tsx";
import { N_, t } from "../i18n.ts";
import { audioRec, audioRecDone, audioRecSkip, setAudioRecSkip, st, startAudioRec, stopAudioRec } from "../state.ts";

const LENGTHS: [number, string][] = [
  [60, N_("1 min")],
  [300, N_("5 min")],
  [900, N_("15 min")],
  [3600, N_("1 h")],
];

const mmss = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

/** While recording: the time (a click stops and saves) and ✕. */
export function AudioRecRunning() {
  const r = audioRec.value;
  if (!r) return null;
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

/** What to record and the lengths (a length starts it; `started` then). */
export function AudioRecBody({ started }: { started: () => void }) {
  return (
    <div class="rec-body">
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
            key={s}
            data-arec={s}
            onClick={() => {
              started();
              startAudioRec(s);
            }}
          >
            ● {t(label)}
          </button>
        ))}
      </div>
      {audioRecDone.value && <p class="dim">{t("Last: {what}", { what: audioRecDone.value })}</p>}
    </div>
  );
}

/** The audio recorder on its own (the phone's sheet). */
export function AudioRecCtl() {
  if (!st.value) return null;
  if (audioRec.value) return <AudioRecRunning />;
  return (
    <Pop cls="iqrec-ctl" title={t("Record the audio to a WAV file")} label={t("Rec")}>
      {(close) => <AudioRecBody started={close} />}
    </Pop>
  );
}
