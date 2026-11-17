// SPDX-License-Identifier: GPL-3.0-or-later
// The browser decoders under one button: a list to turn each on or off
// (several may run at once; each opens its panel beside the waterfall).

import { useEffect, useRef, useState } from "preact/hooks";
import type { Signal } from "@preact/signals";
import { N_, t } from "../i18n.ts";
import { cwOn, digiOn, hfdlOn, imgOn, js8On, jttyOn, pskOn, rttyOn, setCw, setDigi, setHfdl, setImg, setJs8, setJtty, setPsk, setRtty, setSig, setTc, sigOn, tcOn } from "../state.ts";

interface Decoder {
  label: string;
  hint: string;
  on: Signal<boolean>;
  set: (on: boolean) => void;
}

// i18n: hint = one-line description of each browser decoder; mode names (FT8, CW, RTTY, NAVTEX, DSC, PSK31, BPSK, JS8, HFDL, ACARS, SSTV, DCF77, MSF, WWVB, JJY, WWV, WWVH, JTTY) stay in English
export const DECODERS: Decoder[] = [
  { label: "FT8/FT4/WSPR", hint: N_("on the frequency you tune; nothing is reported anywhere"), on: digiOn, set: setDigi },
  { label: "CW", hint: N_("every keyed signal in the passband (Morse)"), on: cwOn, set: setCw },
  { label: "RTTY", hint: N_("FSK/RTTY, NAVTEX and DSC, every parameter found from the signal"), on: rttyOn, set: setRtty },
  { label: "PSK", hint: N_("PSK31, PSK63 and PSK125: every BPSK signal in the passband"), on: pskOn, set: setPsk },
  { label: "JS8", hint: N_("JS8Call: heartbeats, directed messages and chat, every submode (USB)"), on: js8On, set: setJs8 },
  { label: "HFDL", hint: N_("aircraft HF data link: position reports and ACARS (USB)"), on: hfdlOn, set: setHfdl },
  { label: "FAX/SSTV", hint: N_("HF weather charts and SSTV pictures (USB)"), on: imgOn, set: setImg },
  { label: N_("Time"), hint: N_("time-code stations: DCF77, MSF, WWVB, JJY, WWV/WWVH"), on: tcOn, set: setTc },
  { label: N_("Signal ID"), hint: N_("identify the signals in the passband"), on: sigOn, set: setSig },
  { label: "JTTY", hint: N_("WSJT-X's keyboard mode"), on: jttyOn, set: setJtty },
];

export function DecodersCtl() {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  // a click elsewhere or Escape closes the list
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
  const active = DECODERS.filter((d) => d.on.value);
  return (
    <div class="group decoders-ctl" ref={box}>
      <button
        class={`decoders${active.length ? " on" : ""}${open ? " open" : ""}`}
        onClick={() => setOpen(!open)}
        title={active.length ? t("Decoding in this browser: {list}", { list: active.map((d) => t(d.label)).join(", ") }) : t("Decoders that run in this browser")}
      >
        {t("Decoders")}{active.length ? ` ${active.length}` : ""} ▾
      </button>
      {open && (
        <div class="decoders-pop">
          {DECODERS.map((d) => (
            <label title={t(d.hint)}>
              <input type="checkbox" data-dec={d.label} checked={d.on.value} onChange={(e) => d.set(e.currentTarget.checked)} />
              <span>{t(d.label)}</span>
              <span class="dim">{t(d.hint)}</span>
            </label>
          ))}
        </div>
      )}
    </div>
  );
}
