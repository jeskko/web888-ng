// SPDX-License-Identifier: GPL-3.0-or-later
// The panels beside the waterfall under one button: the spots, the map,
// the audio spectrum and the browser decoders, a list to turn each on or
// off (several may be open at once).

import { useEffect, useRef, useState } from "preact/hooks";
import { usePopFocus } from "../a11y.ts";
import type { Signal } from "@preact/signals";
import { N_, t } from "../i18n.ts";
import { afOn, setSigLabels, sigLabelsOn, wideAvail, dxAvail, mapOpen, setAf, showMap, showSpots, spotsAvail, spotsOpen, cwOn, digiOn, hfdlOn, imgOn, js8On, jttyOn, pskOn, rttyOn, setCw, setDigi, setHfdl, setImg, setJs8, setJtty, setPsk, setRtty, setSig, setTc, sigOn, tcOn } from "../state.ts";

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

/** The panels beside the waterfall that aren't decoders. */
function views(): Decoder[] {
  const v: Decoder[] = [];
  if (spotsAvail.value || dxAvail.value) {
    v.push({
      label: N_("Spots"),
      hint: spotsAvail.value ? (dxAvail.value ? N_("Decodes of this receiver's skimmers and live DX spots") : N_("Decodes of this receiver's skimmers")) : N_("Live DX spots"),
      on: spotsOpen,
      set: showSpots,
    });
  }
  v.push({ label: N_("Map"), hint: N_("Map of the stations heard"), on: mapOpen, set: showMap });
  if (wideAvail.value) {
    v.push({
      // i18n: a switch: every signal in the waterfall's view named on the spectrum
      label: N_("Signal labels"),
      hint: N_("Name every signal in the waterfall's view on the spectrum (at 1 MHz wide or narrower): the identifier in this browser, over the receiver's 40 kHz slices in turn (~1 Mbit/s while on)"),
      on: sigLabelsOn,
      set: setSigLabels,
    });
  }
  v.push({ label: "AF", hint: N_("Audio spectrum: the audio's spectrum and waterfall in a panel (in IQ, both sides of the dial)"), on: afOn, set: setAf });
  return v;
}

/** The button names what is open up to this many characters. */
const LABEL_CHARS = 12;

/** Everything that opens beside the waterfall under one button: the
 * spots, the map, the audio spectrum and the browser decoders, each a
 * checkbox (several may be open at once). The button names what is
 * open (two at most, then how many). */
export function PanelsCtl() {
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
  const vs = views();
  const active = [...vs, ...DECODERS].filter((d) => d.on.value);
  const names = active.map((d) => t(d.label));
  const pop = usePopFocus(open);
  const row = (d: Decoder) => (
    <label key={d.label} title={t(d.hint)}>
      <input type="checkbox" data-dec={d.label} checked={d.on.value} onChange={(e) => d.set(e.currentTarget.checked)} />
      <span>{t(d.label)}</span>
      <span class="dim">{t(d.hint)}</span>
    </label>
  );
  return (
    <div class="group decoders-ctl" ref={box}>
      <button
        class={`decoders${active.length ? " on" : ""}${open ? " open" : ""}`}
        aria-expanded={open}
        aria-haspopup="dialog"
        onClick={() => setOpen(!open)}
        title={active.length ? t("Open beside the waterfall: {list}", { list: names.join(", ") }) : t("The spots, the map, the audio spectrum and the decoders that run in this browser, beside the waterfall")}
      >
        {/* i18n: button opening the list of panels beside the waterfall (spots, map, audio spectrum, decoders) */}
        {/* the names while short (the header stays one row), else how many */}
        {active.length && names.join(" ").length <= LABEL_CHARS ? names.join(" ") : `${t("Panels")}${active.length ? ` ${active.length}` : ""}`} ▾
      </button>
      {open && (
        <div class="decoders-pop" ref={pop} role="group" aria-label={t("Panels")}>
          {vs.map(row)}
          {/* i18n: heading in the panel list: the decoders that run in this browser */}
          <div class="decoders-head">{t("Decoders in this browser")}</div>
          {DECODERS.map(row)}
        </div>
      )}
    </div>
  );
}
