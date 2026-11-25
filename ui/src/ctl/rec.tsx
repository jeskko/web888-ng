// SPDX-License-Identifier: GPL-3.0-or-later
// Recording, one button: the audio (a WAV) or the IQ around the dial,
// chosen in its box. A running recording shows its time beside it (a
// click stops and saves); the audio and the IQ can run at once.

import { signal } from "@preact/signals";
import { t } from "../i18n.ts";
import { audioRec, iqRec, st, wideAvail } from "../state.ts";
import { AudioRecBody, AudioRecRunning } from "./audiorec.tsx";
import { IqRecBody, IqRecRunning } from "./iqrec.tsx";
import { Pop } from "./pop.tsx";

type Kind = "audio" | "iq";

const KIND_KEY = "recKind";
const kind = signal<Kind>(((): Kind => {
  try {
    return localStorage.getItem(KIND_KEY) === "iq" ? "iq" : "audio";
  } catch {
    return "audio";
  }
})());

function pick(k: Kind) {
  kind.value = k;
  try {
    localStorage.setItem(KIND_KEY, k);
  } catch {
    /* not kept */
  }
}

export function RecCtl() {
  if (!st.value) return null;
  // what can still start
  const can: Kind[] = [...(audioRec.value ? [] : ["audio" as Kind]), ...(wideAvail.value && !iqRec.value ? ["iq" as Kind] : [])];
  const k = can.includes(kind.value) ? kind.value : can[0];
  return (
    <>
      <AudioRecRunning />
      <IqRecRunning />
      {k && (
        <Pop cls="iqrec-ctl rec-ctl" right title={t("Record the audio to a WAV file, or the IQ around the dial (60 kHz)")} label={t("Rec")}>
          {(close) => (
            <>
              {can.length > 1 && (
                <span class="seg rec-kind">
                  <button class={k === "audio" ? "on" : ""} aria-pressed={k === "audio"} onClick={() => pick("audio")}>
                    {/* i18n: what to record: the audio */}
                    {t("Audio")}
                  </button>
                  <button class={k === "iq" ? "on" : ""} aria-pressed={k === "iq"} onClick={() => pick("iq")}>
                    {/* i18n: what to record: IQ = complex baseband samples around the dial */}
                    IQ
                  </button>
                </span>
              )}
              {k === "audio" ? <AudioRecBody started={close} /> : <IqRecBody started={close} />}
            </>
          )}
        </Pop>
      )}
    </>
  );
}
