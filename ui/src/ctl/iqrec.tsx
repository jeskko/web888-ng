// SPDX-License-Identifier: GPL-3.0-or-later
// IQ recording (src/iqrec.ts): the channelizer bin around the dial, saved
// as a file in this browser. A list to pick the length and the format;
// while recording the button shows the time and stops (and saves) on a
// click.

import { useEffect, useRef, useState } from "preact/hooks";
import { usePopFocus } from "../a11y.ts";
import { N_, t } from "../i18n.ts";
import { iqRec, iqRecDone, iqRecFormat, setIqRecFormat, startIqRec, stopIqRec, wideAvail } from "../state.ts";
import type { RecFormat } from "../iqrec.ts";

const LENGTHS: [number, string][] = [
  [10, N_("10 s")],
  [30, N_("30 s")],
  [60, N_("1 min")],
  [120, N_("2 min")],
  [300, N_("5 min")],
];

const FORMATS: [RecFormat, string, string][] = [
  ["cs16", ".cs16", N_("raw 16-bit I/Q, no header (GNU Radio, numpy)")],
  ["wav", ".wav", N_("stereo 16-bit WAV, I left and Q right (SDR programs)")],
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
  const pop = usePopFocus(open);
  if (!wideAvail.value) return null;
  const r = iqRec.value;
  if (r) {
    const where = r.centre ? t(" of {lo}-{hi} kHz", { lo: Math.round((r.centre - r.rate / 2) / 1000), hi: Math.round((r.centre + r.rate / 2) / 1000) }) : "";
    const lost = r.lost ? t(", {s} s lost", { s: (r.lost / (r.rate || 1)).toFixed(1) }) : "";
    return (
      <div class="group iqrec-ctl">
        <button class="iqrec on" onClick={() => stopIqRec()} title={t("Recording the IQ{where}{lost}: click to stop and save", { where, lost })}>
          ● {r.seconds.toFixed(0)} / {r.maxS} s
        </button>
        <button class="iqrec-x" onClick={() => stopIqRec(false)} title={t("Stop without saving")} aria-label={t("Stop without saving")}>
          ✕
        </button>
      </div>
    );
  }
  return (
    <div class="group iqrec-ctl" ref={box}>
      <button class={`iqrec${open ? " open" : ""}`} aria-expanded={open} onClick={() => setOpen(!open)} title={t("Record the IQ around the dial (60 kHz) to a file")}>
        {/* i18n: Rec = short for Record; IQ = complex baseband samples (in-phase and quadrature) */}
        {t("Rec IQ")} ▾
      </button>
      {open && (
        <div class="iqrec-pop" ref={pop}>
          <p class="dim">
            {/* i18n: channelizer bin = one 60 kHz slice of the spectrum that the receiver's channelizer produces; IQ = complex baseband samples */}
            {t(
              "The channelizer bin around the dial: 60 kHz of IQ (clean within ±21 kHz of its centre), 16-bit, collected in this browser and saved as a file (~0.25 MB a second). The bin stays put while recording.",
            )}
          </p>
          <div class="iqrec-row">
            {FORMATS.map(([f, label, hint]) => (
              <label title={t(hint)}>
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
                ● {t(label)}
              </button>
            ))}
          </div>
          {iqRecDone.value && <p class="dim">{t("Last: {what}", { what: iqRecDone.value })}</p>}
        </div>
      )}
    </div>
  );
}
