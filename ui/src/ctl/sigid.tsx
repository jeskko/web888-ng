// SPDX-License-Identifier: GPL-3.0-or-later
// Signal identification in this browser from this receiver's audio: the
// button that turns it on, and beside the waterfall one row per signal
// in the passband with what it looks like (a small CNN, trained on
// synthetic signals) and, where this page has one, a button that opens
// its decoder.

import { formatKhz } from "../fmt.ts";
import { N_, t } from "../i18n.ts";
import { audioBand } from "../cw.ts";
import { CLASS_INFO, SURE, type SigRow } from "../sigid.ts";
import { closeAsk, openSig, setSig, sigAsk, sigError, sigOn, sigRows, st, whatIs } from "../state.ts";

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
      <td class="r">
        <button onClick={() => whatIs(r.rfLo, r.rfHi)} title={t("What is this? A closer look: its width, tones, keying, symbol rate and frames, measured and explained")} aria-label={t("What is this?")}>
          ?
        </button>{" "}
        {best && info!.opens && (
          <button onClick={() => openSig(r, best.cls)} title={info!.opens === "ssb" || info!.opens === "am" ? t("Tune to it") : t("Open the {mode} decoder", { mode: t(info!.label) })}>
            {info!.opens === "ssb" || info!.opens === "am" ? t("Tune") : t("Decode")}
          </button>
        )}
      </td>
    </tr>
  );
}

// i18n: how sure the "what is this?" answer is
const SURENESS = { sure: N_("the figures fit it"), likely: N_("likely"), unsure: N_("unsure") } as const;

/** "What is this?": the answer about one range, above the signal list. */
function AskCard() {
  const a = sigAsk.value;
  if (!a) return null;
  const d = a.desc;
  const opens = d?.cls ? CLASS_INFO[d.cls].opens : null;
  const speech = opens === "ssb" || opens === "am";
  return (
    <section class="sig-ask" aria-live="polite">
      <div class="sig-ask-bar">
        <strong>{t("What is this?")}</strong>
        <span class="dim num">
          {formatKhz(Math.round(a.rfLo))}–{formatKhz(Math.round(a.rfHi))} kHz
        </span>
        <button class="close" onClick={closeAsk} title={t("Close")} aria-label={t("Close")}>
          ✕
        </button>
      </div>
      {d === undefined ? (
        <p class="dim">{a.retuned ? t("Tuned to it; listening for {n} s…", { n: a.left }) : t("Listening for {n} s…", { n: a.left })}</p>
      ) : d === null ? (
        <>
          <p>{t("Nothing stands out of the noise there.")}</p>
          <button onClick={() => whatIs(a.rfLo, a.rfHi)}>{t("Again")}</button>
        </>
      ) : (
        <>
          <p class="sig-ask-title">
            <strong class={d.sure}>{d.title}</strong> <span class="dim">({t(SURENESS[d.sure])})</span>
          </p>
          {d.text.map((x, i) => (
            <p key={i}>{x}</p>
          ))}
          <div class="sig-ask-do">
            {d.cls && opens && (
              <button onClick={() => openSig({ id: 0, rf: d.rf, rfLo: d.rfLo, rfHi: d.rfHi, hz: 0, db: 0, top: [] }, d.cls!)}>
                {speech ? t("Tune") : t("Decode")}
              </button>
            )}
            <button onClick={() => whatIs(a.rfLo, a.rfHi)} title={t("Measure it again from the next 4 seconds")}>
              {t("Again")}
            </button>
          </div>
          <details>
            <summary>{t("Measured")}</summary>
            <table class="sig-facts">
              <tbody>
                {d.facts.map(([k, v]) => (
                  <tr key={k}>
                    <th>{k}</th>
                    <td class="num">{v}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </details>
        </>
      )}
    </section>
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
        <AskCard />
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
        <p class="dim">{t("For a closer look at one signal: its ? button, or shift-drag across it on the waterfall.")}</p>
      </div>
    </aside>
  );
}
