// SPDX-License-Identifier: GPL-3.0-or-later
// Signal identification in this browser from this receiver's audio: the
// button that turns it on, and beside the waterfall one row per signal
// in the passband with what it looks like (a small CNN, trained on
// synthetic signals) and, where this page has one, a button that opens
// its decoder.

import { formatKhz } from "../fmt.ts";
import { t } from "../i18n.ts";
import { audioBand } from "../cw.ts";
import { CLASS_INFO, SURE, type SigRow } from "../sigid.ts";
import { openSig, setSig, sigError, sigOn, sigRows, st } from "../state.ts";

export function SigCtl() {
  const on = sigOn.value;
  return (
    <div class="group">
      <button class={on ? "on" : ""} aria-pressed={!!(on)} onClick={() => setSig(!on)} title={t("Identify the signals in the passband (CW, RTTY, FT8, SSTV, fax, voice, data modems…), in this browser")}>
        ID
      </button>
    </div>
  );
}

function Row({ r }: { r: SigRow }) {
  const best = r.top[0];
  const info = best && CLASS_INFO[best.cls];
  const sure = best && best.p >= SURE;
  const width = r.rfHi - r.rfLo;
  return (
    <tr title={t("{hz} Hz audio, {width} Hz wide; {classes}", { hz: Math.round(r.hz), width: Math.round(width), classes: r.top.map((c) => `${t(CLASS_INFO[c.cls].label)} ${Math.round(100 * c.p)} %`).join(", ") })}>
      <td class="r">{formatKhz(Math.round(r.rf)).slice(0, -2)}</td>
      <td class="r num">{r.db.toFixed(0)}</td>
      <td>
        {!best ? (
          <span class="dim">…</span>
        ) : (
          <>
            <span class={sure ? "" : "dim"} title={t(info!.hint)}>
              {t(info!.label)}
            </span>{" "}
            <span class="dim num">{Math.round(100 * best.p)} %</span>
            {!sure && r.top[1] && <span class="dim"> / {t(CLASS_INFO[r.top[1].cls].label)}?</span>}
          </>
        )}
      </td>
      <td>
        {best && info!.opens && (
          <button onClick={() => openSig(r, best.cls)} title={info!.opens === "ssb" || info!.opens === "am" ? t("Tune to it") : t("Open the {mode} decoder", { mode: t(info!.label) })}>
            {info!.opens === "ssb" || info!.opens === "am" ? t("Tune") : t("Decode")}
          </button>
        )}
      </td>
    </tr>
  );
}

export function SigPanel() {
  if (!sigOn.value) return null;
  const rows = sigRows.value;
  const s = st.value;
  const band = s && audioBand(s.mode, s.lo, s.hi, s.pitch);
  return (
    <aside class="spots sigid">
      <div class="spots-bar">
        <strong>{t("Signals")}</strong>
        <span class="dim num">{rows.length}</span>
        <button class="close" onClick={() => setSig(false)} title={t("Stop identifying")} aria-label={t("Stop identifying")}>
          ✕
        </button>
      </div>
      <div class="spots-list">
        {sigError.value && <p class="warn" role="alert">{sigError.value}</p>}
        {s && !band && <p class="dim">{t("Signals are identified in CW, USB and LSB.")}</p>}
        {rows.length === 0 ? (
          <p class="dim">{t("Nothing yet: every signal in the passband is found and identified in turn from the last 4 seconds of audio, one every 2 s. A wide passband (USB, 3 kHz) shows several at once.")}</p>
        ) : (
          <table>
            <thead>
              <tr>
                <th class="r">kHz</th>
                <th class="r">dB</th>
                <th>{t("Looks like")}</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <Row key={r.id} r={r} />
              ))}
            </tbody>
          </table>
        )}
        <p class="dim">{t("Trained on synthetic signals; a guess, not a decode.")}</p>
      </div>
    </aside>
  );
}
