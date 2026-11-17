// SPDX-License-Identifier: GPL-3.0-or-later
// PSK31/63/125 decoded in this browser from this receiver's audio: beside
// the waterfall, every BPSK signal in the passband with its rate and how
// cleanly it reads, and its text (characters read weakly dimmed).

import { formatKhz } from "../fmt.ts";
import { t } from "../i18n.ts";
import { audioBand } from "../cw.ts";
import { PSK_STATIONS, pskName } from "../psk.ts";
import { textRuns } from "../rtty.ts";
import { clearPsk, pickPsk, pskError, pskOn, pskSignals, setPsk, st } from "../state.ts";

/** Characters shown per signal (the newest). */
const SHOWN = 1500;

export function PskPanel() {
  if (!pskOn.value) return null;
  const sigs = pskSignals.value;
  const s = st.value;
  const band = s && audioBand(s.mode, s.lo, s.hi, s.pitch);
  return (
    <aside class="spots psk">
      <div class="spots-bar">
        <strong>PSK</strong>
        <select value="" onChange={(e) => pickPsk(Number((e.target as HTMLSelectElement).value))} title={t("The usual PSK31 frequencies (USB)")}>
          <option value="" disabled>
            {t("Band…")}
          </option>
          {PSK_STATIONS.map((x) => (
            <option value={String(x.hz)}>{x.name}</option>
          ))}
        </select>
        <span class="dim num">{sigs.filter((x) => x.active).length}</span>
        <button onClick={clearPsk} title={t("Clear the text")}>
          {t("Clear")}
        </button>
        <button class="close" onClick={() => setPsk(false)} title={t("Stop decoding")}>
          ✕
        </button>
      </div>
      <div class="spots-list">
        {pskError.value && <p class="warn">{pskError.value}</p>}
        {s && !band && <p class="dim">{t("PSK is decoded in USB, LSB and CW.")}</p>}
        {sigs.length === 0 ? (
          <p class="dim">{t("Nothing yet: every PSK31, PSK63 and PSK125 signal in the passband is found (its carrier and rate from the signal) and decoded. A wide passband (USB, 3 kHz) reads several at once.")}</p>
        ) : (
          sigs.map((x) => (
            <div class={`rtty-sig${x.active ? "" : " dim"}`}>
              <div class="rtty-head">
                <span class="num">{formatKhz(Math.round(x.rf)).slice(0, -2)}</span> <span>{pskName(x.baud)}</span>
                {/* i18n: phase changes = the BPSK symbols (carrier phase flips between 0 and 180 degrees) */}
                <span class="dim num" title={t("How cleanly its phase changes read: 100 % clean, ~64 % noise")}>
                  {" "}
                  · {Math.round(100 * x.q)} %{x.open ? "" : ` · ${t("quiet")}`}
                </span>
              </div>
              {x.text && (
                <pre class="rtty-text">
                  {textRuns(x.text, x.weak, SHOWN).map((r) =>
                    r.weak ? (
                      <span class="weak" title={t("Read weakly: a phase change near 90°, it may have been another character")}>
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
