// SPDX-License-Identifier: GPL-3.0-or-later
// IQ recording (src/iqrec.ts): the channelizer bin around the dial, saved
// as a file in this browser. A list to pick the length and the format;
// while recording the button shows the time and stops (and saves) on a
// click.

import { useEffect, useRef, useState } from "preact/hooks";
import { iqRec, iqRecDone, iqRecFormat, setIqRecFormat, startIqRec, stopIqRec, wideAvail } from "../state.ts";
import type { RecFormat } from "../iqrec.ts";

const LENGTHS: [number, string][] = [
  [10, "10 s"],
  [30, "30 s"],
  [60, "1 min"],
  [120, "2 min"],
  [300, "5 min"],
];

const FORMATS: [RecFormat, string, string][] = [
  ["cs16", ".cs16", "raw 16-bit I/Q, no header (GNU Radio, numpy)"],
  ["wav", ".wav", "stereo 16-bit WAV, I left and Q right (SDR programs)"],
];

export function IqRecCtl() {
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
  if (!wideAvail.value) return null;
  const r = iqRec.value;
  if (r) {
    const where = r.centre ? ` of ${Math.round((r.centre - r.rate / 2) / 1000)}-${Math.round((r.centre + r.rate / 2) / 1000)} kHz` : "";
    const lost = r.lost ? `, ${(r.lost / (r.rate || 1)).toFixed(1)} s lost` : "";
    return (
      <div class="group iqrec-ctl">
        <button class="iqrec on" onClick={() => stopIqRec()} title={`Recording the IQ${where}${lost}: click to stop and save`}>
          ● {r.seconds.toFixed(0)} / {r.maxS} s
        </button>
        <button class="iqrec-x" onClick={() => stopIqRec(false)} title="Stop without saving">
          ✕
        </button>
      </div>
    );
  }
  return (
    <div class="group iqrec-ctl" ref={box}>
      <button class={`iqrec${open ? " open" : ""}`} onClick={() => setOpen(!open)} title="Record the IQ around the dial (60 kHz) to a file">
        Rec IQ ▾
      </button>
      {open && (
        <div class="iqrec-pop">
          <p class="dim">
            The channelizer bin around the dial: 60 kHz of IQ (clean within ±21 kHz of its centre), 16-bit, collected in this browser
            and saved as a file (~0.25 MB a second). The bin stays put while recording.
          </p>
          <div class="iqrec-row">
            {FORMATS.map(([f, label, hint]) => (
              <label title={hint}>
                <input type="radio" name="iqrec-format" checked={iqRecFormat.value === f} onChange={() => setIqRecFormat(f)} />
                {label}
              </label>
            ))}
          </div>
          <div class="iqrec-row">
            {LENGTHS.map(([s, label]) => (
              <button
                data-rec={s}
                onClick={() => {
                  setOpen(false);
                  startIqRec(s);
                }}
              >
                ● {label}
              </button>
            ))}
          </div>
          {iqRecDone.value && <p class="dim">Last: {iqRecDone.value}</p>}
        </div>
      )}
    </div>
  );
}
