// SPDX-License-Identifier: GPL-3.0-or-later
// The browser decoders under one button: a list to turn each on or off
// (several may run at once; each opens its panel beside the waterfall).

import { useEffect, useRef, useState } from "preact/hooks";
import type { Signal } from "@preact/signals";
import { cwOn, digiOn, imgOn, jttyOn, pskOn, rttyOn, setCw, setDigi, setImg, setJtty, setPsk, setRtty, setSig, sigOn } from "../state.ts";

interface Decoder {
  label: string;
  hint: string;
  on: Signal<boolean>;
  set: (on: boolean) => void;
}

export const DECODERS: Decoder[] = [
  { label: "FT8/FT4/WSPR", hint: "on the frequency you tune; nothing is reported anywhere", on: digiOn, set: setDigi },
  { label: "CW", hint: "every keyed signal in the passband (Morse)", on: cwOn, set: setCw },
  { label: "RTTY", hint: "FSK/RTTY and NAVTEX, every parameter found from the signal", on: rttyOn, set: setRtty },
  { label: "PSK", hint: "PSK31, PSK63 and PSK125: every BPSK signal in the passband", on: pskOn, set: setPsk },
  { label: "FAX/SSTV", hint: "HF weather charts and SSTV pictures (USB)", on: imgOn, set: setImg },
  { label: "Signal ID", hint: "identify the signals in the passband", on: sigOn, set: setSig },
  { label: "JTTY", hint: "WSJT-X's keyboard mode", on: jttyOn, set: setJtty },
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
        title={active.length ? `Decoding in this browser: ${active.map((d) => d.label).join(", ")}` : "Decoders that run in this browser"}
      >
        Decoders{active.length ? ` ${active.length}` : ""} ▾
      </button>
      {open && (
        <div class="decoders-pop">
          {DECODERS.map((d) => (
            <label title={d.hint}>
              <input type="checkbox" data-dec={d.label} checked={d.on.value} onChange={(e) => d.set(e.currentTarget.checked)} />
              <span>{d.label}</span>
              <span class="dim">{d.hint}</span>
            </label>
          ))}
        </div>
      )}
    </div>
  );
}
