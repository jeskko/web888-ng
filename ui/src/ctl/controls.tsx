// SPDX-License-Identifier: GPL-3.0-or-later
// Receiver controls: frequency display, mode, bandwidth, AGC/gain,
// volume, zoom, waterfall contrast, S-meter and the status bar.

import { useSignal } from "@preact/signals";
import { bandwidths, formatKhz, parseFreq } from "../fmt.ts";
import { MODES, wfSpan, type AgcSpeed } from "../proto.ts";
import {
  audioGaps,
  lease,
  listenTo,
  notice,
  setShare,
  users,
  audioOn,
  audioStats,
  conn,
  connNote,
  latencyMs,
  muted,
  overloadAt,
  radio,
  range,
  rssi,
  server,
  setAgc,
  setAudio,
  setMode,
  setPassband,
  setView,
  setVolume,
  st,
  status,
  tune,
  volume,
} from "../state.ts";

/** Dial in kHz; wheel over a digit steps that digit, click to type. */
export function FreqDisplay() {
  const editing = useSignal(false);
  const s = st.value;
  if (!s) return <div class="freq">—</div>;
  if (editing.value) {
    return (
      <input
        class="freq freq-input"
        autoFocus
        placeholder="kHz, or 7.074M"
        onKeyDown={(e) => {
          const el = e.currentTarget;
          if (e.key === "Enter") {
            const f = parseFreq(el.value);
            if (f !== null) tune(f);
            editing.value = false;
          } else if (e.key === "Escape") editing.value = false;
        }}
        onBlur={() => (editing.value = false)}
      />
    );
  }
  // digits with their place value (Hz): "7 074.000" -> 7 MHz ... 1 Hz
  const text = formatKhz(s.hz);
  const intDigits = text.replace(/\..*/, "").replace(/ /g, "").length;
  let place = 10 ** (intDigits + 2);
  return (
    <div class="freq" title="Wheel over a digit to step it, click to type" onClick={() => (editing.value = true)}>
      {[...text].map((ch, i) => {
        if (ch === " " || ch === ".") return <span key={i} class="sep">{ch === "." ? "." : " "}</span>;
        const p = place;
        place /= 10;
        return (
          <span
            key={i}
            class="digit"
            onWheel={(e) => {
              e.preventDefault();
              tune(s.hz + (e.deltaY < 0 ? p : -p));
            }}
          >
            {ch}
          </span>
        );
      })}
      <span class="unit">kHz</span>
    </div>
  );
}

export function ModeBar() {
  const s = st.value;
  return (
    <div class="group">
      {MODES.map((m) => (
        <button key={m} class={s?.mode === m ? "on" : ""} onClick={() => setMode(m)} title={`${m.toUpperCase()} (key ${m[0]})`}>
          {m.toUpperCase()}
        </button>
      ))}
    </div>
  );
}

export function Bandwidth() {
  const s = st.value;
  if (!s) return null;
  return (
    <div class="group" title="Passband">
      {bandwidths(s.mode).map((b) => (
        <button key={b.label} class={s.lo === b.lo && s.hi === b.hi ? "on" : ""} onClick={() => setPassband(b.lo, b.hi)}>
          {b.label}
        </button>
      ))}
    </div>
  );
}

export function AudioCtl() {
  const s = st.value;
  if (!s) return null;
  return (
    <div class="group">
      <button class={s.agc ? "on" : ""} onClick={() => setAgc(!s.agc)} title="Automatic gain control (AM: follows the carrier)">
        AGC
      </button>
      {s.agc && s.mode !== "am" && (
        <span class="seg" title="AGC recovery: fast, medium, slow">
          {(["fast", "medium", "slow"] as AgcSpeed[]).map((v) => (
            <button key={v} class={s.agc_speed === v ? "on" : ""} onClick={() => setAudio({ agc_speed: v })}>
              {v[0].toUpperCase()}
            </button>
          ))}
        </span>
      )}
      <label title={s.agc ? "Maximum AGC gain" : "Fixed gain"}>
        {s.agc ? "max" : "gain"}
        <input
          type="range"
          min={0}
          max={100}
          value={s.gain_db}
          onInput={(e) => setAgc(s.agc, Number(e.currentTarget.value))}
        />
        <span class="num">{s.gain_db.toFixed(0)} dB</span>
      </label>
      <button class={muted.value ? "on" : ""} onClick={() => setVolume(volume.value, !muted.value)} title="Mute">
        {muted.value ? "Muted" : "Vol"}
      </button>
      <input
        type="range"
        min={0}
        max={1}
        step={0.01}
        value={volume.value}
        onInput={(e) => setVolume(Number(e.currentTarget.value), false)}
        title="Volume"
      />
    </div>
  );
}

/** Last levels of the noise blanker and noise reduction, for turning
 * them back on. */
const nbLevel = { v: 8 };
const nrLevel = { v: 0.6 };

/** Noise blanker, noise reduction, squelch. */
export function DspCtl() {
  const s = st.value;
  if (!s) return null;
  if (s.nb > 0) nbLevel.v = s.nb;
  if (s.nr > 0) nrLevel.v = s.nr;
  return (
    <div class="group">
      <button
        class={s.nb > 0 ? "on" : ""}
        onClick={() => setAudio({ nb: s.nb > 0 ? 0 : nbLevel.v })}
        title="Noise blanker: removes short impulses (ignition, switching noise)"
      >
        NB
      </button>
      {s.nb > 0 && (
        <input
          type="range"
          min={3}
          max={30}
          step={0.5}
          value={33 - s.nb}
          onInput={(e) => setAudio({ nb: 33 - Number(e.currentTarget.value) })}
          title={`Blanker sensitivity: impulses over ${s.nb.toFixed(1)} x the average`}
        />
      )}
      <button
        class={s.nr > 0 ? "on" : ""}
        onClick={() => setAudio({ nr: s.nr > 0 ? 0 : nrLevel.v })}
        title="Noise reduction: lowers steady background noise"
      >
        NR
      </button>
      {s.nr > 0 && (
        <input
          type="range"
          min={0.1}
          max={1}
          step={0.05}
          value={s.nr}
          onInput={(e) => setAudio({ nr: Number(e.currentTarget.value) })}
          title={`Noise reduction strength ${(s.nr * 100).toFixed(0)} %`}
        />
      )}
      <button
        class={s.squelch ? "on" : ""}
        onClick={() => setAudio({ squelch: !s.squelch })}
        title="Squelch: mute while the signal is below the threshold (marked on the S-meter)"
      >
        SQL
      </button>
      {s.squelch && (
        <>
          <input
            type="range"
            min={-140}
            max={-20}
            value={s.squelch_db}
            onInput={(e) => setAudio({ squelch_db: Number(e.currentTarget.value) })}
            title="Squelch threshold"
          />
          <span class="num">{s.squelch_db.toFixed(0)} dB</span>
        </>
      )}
    </div>
  );
}

export function ZoomCtl() {
  const s = st.value;
  const r = radio.value;
  if (!s || !r) return null;
  const zoom = (decim: number) => setView({ ...s.wf, decim });
  const full = () => {
    let decim = r.wf_decim_max;
    while (decim > r.wf_decim_min && r.adc_hz / decim < r.rf_hi - r.rf_lo) decim /= 2;
    // the whole range, or around the dial if the widest span can't cover it
    const covers = r.adc_hz / decim >= r.rf_hi - r.rf_lo;
    setView({ ...s.wf, decim, center_hz: covers ? (r.rf_lo + r.rf_hi) / 2 : s.hz });
  };
  const span = wfSpan(r.adc_hz, s.wf.decim);
  return (
    <div class="group">
      <button onClick={() => zoom(s.wf.decim / 2)} disabled={s.wf.decim <= r.wf_decim_min} title="Zoom out (wheel)">
        −
      </button>
      <span class="num" title="Waterfall span">
        {span >= 1e6 ? `${(span / 1e6).toFixed(2)} MHz` : `${(span / 1e3).toFixed(1)} kHz`}
      </span>
      <button onClick={() => zoom(s.wf.decim * 2)} disabled={s.wf.decim >= r.wf_decim_max} title="Zoom in (wheel)">
        +
      </button>
      <button onClick={() => setView({ ...s.wf, center_hz: s.hz })} title="Centre the waterfall on the dial">
        Centre
      </button>
      <button onClick={full} title="Whole range">
        Full
      </button>
    </div>
  );
}

/** Whether others see this user's frequency and may listen along. */
export function ShareCtl() {
  const s = st.value;
  if (!s) return null;
  return (
    <div class="group">
      <button
        class={s.share ? "on" : ""}
        onClick={() => setShare(!s.share)}
        title={
          s.share
            ? "Others see your frequency on their scale and may listen along while they wait for a receiver. Click to stop."
            : "Your frequency is private. Click to share it."
        }
      >
        {s.share ? "Shared" : "Private"}
      </button>
    </div>
  );
}

/** The receiver's state when it isn't simply ours: waiting in the queue,
 * listening along, or a message from the server. */
export function LeaseBanner() {
  const s = st.value;
  const l = lease.value;
  const n = notice.value;
  let body = null;
  if (s?.listen != null) {
    body = l?.granted ? (
      <>
        <span>Your receiver is ready.</span>
        <button onClick={() => listenTo(null)}>Switch to it</button>
      </>
    ) : (
      <>
        <span>
          Listening along with user {s.listen} (waiting for a receiver of your own, #{l?.position ?? "?"} in the queue).
        </span>
        <button onClick={() => listenTo(null)}>Stop</button>
      </>
    );
  } else if (l && !l.granted) {
    body = (
      <span>
        All receivers are in use: you are #{l.position} in the queue, waterfall only until then.
        {users.value.length > 0 && " Click a green marker on the scale to listen along with another user."}
      </span>
    );
  }
  if (!body && !n) return null;
  return (
    <div class="banner">
      {body}
      {n && <span class="warn">{n}</span>}
    </div>
  );
}

export function ContrastCtl() {
  const r = range.value;
  return (
    <div class="group" title="Waterfall colour range (dBFS)">
      <button class={r.auto ? "on" : ""} onClick={() => (range.value = { ...r, auto: !r.auto })}>
        Auto
      </button>
      <label>
        floor
        <input
          type="range"
          min={-170}
          max={-20}
          value={r.min}
          onInput={(e) => (range.value = { ...r, min: Number(e.currentTarget.value), auto: false })}
        />
      </label>
      <label>
        top
        <input
          type="range"
          min={-150}
          max={0}
          value={r.max}
          onInput={(e) => (range.value = { ...r, max: Number(e.currentTarget.value), auto: false })}
        />
      </label>
      <span class="num">
        {r.min}…{r.max}
      </span>
    </div>
  );
}

export function SMeter() {
  const p = rssi.value;
  const lo = -140;
  const hi = -20;
  const pos = (db: number) => Math.min(1, Math.max(0, (db - lo) / (hi - lo)));
  const t = pos(p);
  const over = performance.now() - overloadAt.value < 1000;
  const s = st.value;
  return (
    <div class="smeter" title="Channel power before AGC (dBFS)">
      <div class="bar">
        <div class="fill" style={{ width: `${(t * 100).toFixed(1)}%` }} />
        {s?.squelch && <div class="sql" style={{ left: `${(pos(s.squelch_db) * 100).toFixed(1)}%` }} title="Squelch threshold" />}
      </div>
      <span class="num">{p > -199 ? `${p.toFixed(1)} dBFS` : "—"}</span>
      <span class={over ? "lamp lit" : "lamp"} title="ADC overload">
        OVL
      </span>
    </div>
  );
}

export function StatusBar() {
  const c = conn.value;
  const s = status.value;
  const a = audioStats.value;
  const r = radio.value;
  return (
    <div class="status">
      <span class={`conn ${c}`}>{c}</span>
      {connNote.value && <span>{connNote.value}</span>}
      {r && (
        <span>
          {r.backend} · {(r.rx_rate / 1e3).toFixed(0)} kS/s
        </span>
      )}
      {latencyMs.value !== null && <span>rtt {latencyMs.value.toFixed(0)} ms</span>}
      {audioOn.value && a && (
        <span title="Audio buffer, underruns, playback trim">
          buf {a.fillMs.toFixed(0)} ms · {a.underruns} underruns · {((a.trim - 1) * 1e6).toFixed(0)} ppm
        </span>
      )}
      {s && (
        <span title="Frames dropped (audio / waterfall), audio gaps">
          drops {s.audio_drops}/{s.wf_drops} · gaps {audioGaps.value}
        </span>
      )}
      {s && !s.hw_rate_ok && <span class="warn">hardware falling behind</span>}
      {s && s.clock_ppm !== 0 && <span>clock {s.clock_ppm.toFixed(3)} ppm</span>}
      <span class="right">{server.value}</span>
    </div>
  );
}
