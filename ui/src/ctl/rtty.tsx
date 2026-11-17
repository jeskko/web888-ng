// SPDX-License-Identifier: GPL-3.0-or-later
// FSK / RTTY decoded in this browser from this receiver's audio: the
// button, and beside the waterfall each two-tone signal in the passband
// with the parameters worked out from it (rate, shift, framing, polarity,
// character set) and its text, characters read weakly dimmed; signals
// identified but not decodable (reversals, other codes) are listed as
// such.

import { formatKhz } from "../fmt.ts";
import { t } from "../i18n.ts";
import { audioBand } from "../cw.ts";
import { describe, navtex, NAVTEX_SUBJECTS, RTTY_STATIONS, textRuns } from "../rtty.ts";
import { clearRtty, pickRtty, rttyError, rttyOn, rttySignals, rttyWide, setRtty, setRttyWide, st, WIDE_HALF, wideAvail, wideCentre } from "../state.ts";

/** Characters shown per signal (the newest). */
const SHOWN = 1500;

export function RttyCtl() {
  const on = rttyOn.value;
  return (
    <div class="group">
      <button
        class={on ? "on" : ""}
        onClick={() => setRtty(!on)}
        // i18n: FSK = frequency-shift keying (two tones); rate = baud; shift = tone spacing, Hz; framing = start, data, parity and stop bits of each character
        title={t("Decode FSK/RTTY (and NAVTEX) from this receiver's audio, in this browser: rate, shift, framing and character set found from the signal")}
      >
        RTTY
      </button>
    </div>
  );
}

export function RttyPanel() {
  if (!rttyOn.value) return null;
  const sigs = rttySignals.value;
  const s = st.value;
  const band = s && audioBand(s.mode, s.lo, s.hi, s.pitch);
  return (
    <aside class="spots rtty">
      <div class="spots-bar">
        <strong>RTTY</strong>
        <select value="" onChange={(e) => pickRtty(Number((e.target as HTMLSelectElement).value))} title={t("Stations to tune to (USB, the signal at 1 kHz)")}>
          <option value="" disabled>
            {t("Station…")}
          </option>
          {RTTY_STATIONS.map((x) => (
            <option value={String(x.hz)}>{x.name}</option>
          ))}
        </select>
        <span class="dim num">{sigs.filter((x) => x.active && x.locked).length}</span>
        {wideAvail.value && (
          <button
            class={rttyWide.value ? "on" : ""}
            onClick={() => setRttyWide(!rttyWide.value)}
            // i18n: Wide = skim a 40 kHz slice of spectrum (a channelizer bin of the receiver's filter bank) instead of the narrow audio passband
            title={t("Skim 40 kHz around the dial (a channelizer bin straight from the receiver, ~1 Mbit/s) instead of the audio passband")}
          >
            {/* i18n: Wide = skim a 40 kHz slice of spectrum (a channelizer bin) instead of the narrow audio passband */}
            {t("Wide")}
          </button>
        )}
        <button onClick={clearRtty} title={t("Clear the text")}>
          {t("Clear")}
        </button>
        <button class="close" onClick={() => setRtty(false)} title={t("Stop decoding")}>
          ✕
        </button>
      </div>
      <div class="spots-list">
        {rttyError.value && <p class="warn">{rttyError.value}</p>}
        {wideAvail.value && rttyWide.value ? (
          <p class="dim">{wideCentre.value ? t("Skimming {lo}–{hi} kHz", { lo: formatKhz(wideCentre.value - WIDE_HALF).slice(0, -2), hi: formatKhz(wideCentre.value + WIDE_HALF).slice(0, -2) }) : t("Waiting for the wide IQ…")}</p>
        ) : (
          s && !band && <p class="dim">{t("RTTY is decoded in USB, LSB and CW.")}</p>
        )}
        {sigs.length === 0 ? (
          <p class="dim">
            {/* i18n: rate = baud (symbol rate); shift = frequency shift between the two tones; polarity = which tone is mark; character set = ITA2, ASCII etc. */}
            {t("Nothing yet: every two-tone FSK signal in the passband is found, its rate, shift, framing (data bits, parity, stop bits), polarity and character set worked out, and decoded. A signal needs a few seconds before it locks.")}
          </p>
        ) : (
          sigs.map((x) => (
            <div class={`rtty-sig${x.active ? "" : " dim"}`}>
              <div class="rtty-head">
                <span class="num">{formatKhz(Math.round(x.rf)).slice(0, -2)}</span> <span>{s ? describe(x.p, s.mode) : ""}</span>
                {x.locked ? (
                  <span class="dim num">
                    {" "}
                    · {x.snr.toFixed(0)} dB{x.p.mode === "async" ? ` · ${t("{pct} % framing errors", { pct: (100 * x.fe).toFixed(0) })}` : ""}
                  </span>
                ) : (
                  <span class="dim"> · {t("identified, not decoded")}</span>
                )}
              </div>
              {x.p.mode === "sitor" &&
                navtex(x.text).map((m) => (
                  <details class="navtex">
                    <summary>
                      NAVTEX {m.station}
                      {m.subject}
                      {m.number} · {NAVTEX_SUBJECTS[m.subject] ? t(NAVTEX_SUBJECTS[m.subject]) : "?"}
                    </summary>
                    <pre class="rtty-text">{m.text}</pre>
                  </details>
                ))}
              {x.text && (
                <pre class="rtty-text">
                  {textRuns(x.text, x.weak, SHOWN).map((r) =>
                    r.weak ? (
                      <span class="weak" title={t("Read weakly: a bit near the threshold, it may have been another character")}>
                        {r.s}
                      </span>
                    ) : (
                      r.s
                    ),
                  )}
                </pre>
              )}
            </div>
          ))
        )}
      </div>
    </aside>
  );
}
