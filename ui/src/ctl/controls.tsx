// SPDX-License-Identifier: GPL-3.0-or-later
// Receiver controls: frequency display, mode, bandwidth, AGC/gain,
// volume, zoom, waterfall contrast, S-meter and the status bar.

import { signal, useSignal } from "@preact/signals";
import { rowAction, usePopFocus } from "../a11y.ts";
import { useEffect, useRef } from "preact/hooks";
import { N_, t, tc, tn, tx } from "../i18n.ts";
import { setFloor, setTop } from "../view/range.ts";
import { helpOpen } from "./keys.tsx";
import { infoOpen } from "./info.tsx";
import { Drop } from "./drop.tsx";
import { bandwidths, formatKhz, parseFreq, shownOffset, stepFor, stepLabel, STEPS, stepTo } from "../fmt.ts";
import { isAm, wfSpan, type AgcSpeed, type Mode } from "../proto.ts";
import {
  audioGaps,
  nnApplies,
  nnChoice,
  nnKind,
  nnKindModels,
  nnOn,
  nnStrength,
  audioRates,
  cycleAm,
  followable,
  lease,
  myName,
  pickSkimmer,
  pickUser,
  sessionId,
  skimmers,
  usersOpen,
  listenTo,
  notice,
  users,
  audioOn,
  audioStats,
  conn,
  connNote,
  latencyMs,
  muted,
  overloadAt,
  player,
  radio,
  range,
  rssi,
  server,
  setAgc,
  setAudio,
  setMode,
  setPassband,
  setStereo,
  setTuneStep,
  setView,
  setVolume,
  st,
  status,
  stepOf,
  stereo,
  tune,
  tuneInView,
  tuneStep,
  volume,
  audioKbps,
  codecNow,
  codecPref,
  opusDec,
  serverCodecs,
  setCodecPref,
  type CodecPref,
  wfKbps,
  paneHooks,
  peakHold,
  setPeakHold,
  guestOpen,
  guestLogin,
  afOn,
  setAf,
} from "../state.ts";

/** The frequency entry is open (a click on the dial, or key g). */
export const freqEdit = signal(false);

/** Dial in kHz; wheel over a digit steps that digit, click to type. */
export function FreqDisplay() {
  const editing = freqEdit;
  const s = st.value;
  if (!s) return <div class="freq">—</div>;
  if (editing.value) {
    return (
      <input
        class="freq freq-input"
        autoFocus
        aria-label={t("Frequency to tune, in kHz (7074) or MHz (7.074M); Enter tunes, Escape cancels")}
        placeholder={t("kHz, or 7.074M")}
        onKeyDown={(e) => {
          const el = e.currentTarget;
          if (e.key === "Enter") {
            const f = parseFreq(el.value);
            // a typed frequency off the waterfall brings the view along
            if (f !== null) tuneInView(f - shownOffset());
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
    <button
      type="button"
      class="freq"
      title={t("Wheel over a digit to step it, click to type")}
      aria-label={t("Frequency {f} kHz. Press to type another (key g)", { f: text })}
      onClick={() => (editing.value = true)}
    >
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
    </button>
  );
}

/** Tuning step: the arrows here, the arrow keys and shift+wheel step by
 * it (to its grid), and a click on the waterfall lands on it. */
export function StepCtl() {
  const s = st.value;
  if (!s) return null;
  const step = stepOf(s.mode);
  return (
    <div class="group" title={t("Tuning step: buttons, arrow keys (shift: x10) and shift+wheel step to its grid; clicks on the waterfall snap to it")}>
      <button onClick={() => tune(stepTo(s.hz, step, -1))} aria-label={t("Tune down one step (key ←)")}>◀</button>
      <select aria-label={t("Tuning step")} value={tuneStep.value} onChange={(e) => setTuneStep(Number(e.currentTarget.value))}>
        {STEPS.map((v) => (
          <option key={v} value={v}>
            {v === 0 ? t("auto ({step})", { step: stepLabel(stepFor(s.mode)) }) : stepLabel(v)}
          </option>
        ))}
      </select>
      <button onClick={() => tune(stepTo(s.hz, step, 1))} aria-label={t("Tune up one step (key →)")}>▶</button>
    </div>
  );
}

const AM_TITLES: Record<string, string> = {
  am: N_("AM, envelope detector"),
  sam: N_("Synchronous AM: locked to the carrier, both sidebands (less distortion in fading)"),
  sal: N_("Synchronous AM, lower sideband only (away from interference above)"),
  sar: N_("Synchronous AM, upper sideband only (away from interference below)"),
  sas: N_("Synchronous AM stereo: lower sideband left, upper right"),
};

export function ModeBar() {
  const s = st.value;
  const am = s && isAm(s.mode) ? s.mode : null;
  return (
    <div class="group">
      {(["usb", "lsb", "cw"] as Mode[]).map((m) => (
        <button key={m} class={s?.mode === m ? "on" : ""} aria-pressed={!!(s?.mode === m)} onClick={() => setMode(m)} title={t("{mode} (key {key})", { mode: m.toUpperCase(), key: m[0] })}>
          {m.toUpperCase()}
        </button>
      ))}
      <button
        class={am ? "on am" : "am"} aria-pressed={!!am}
        onClick={cycleAm}
        title={t("{what}. Click again (or key a) for AM, SAM, SAL, SAR, SAS in turn.", { what: t(AM_TITLES[am ?? "am"]) })}
      >
        {(am ?? "am").toUpperCase()}
      </button>
      <button
        class={s?.mode === "nbfm" ? "on" : ""} aria-pressed={!!(s?.mode === "nbfm")}
        onClick={() => setMode("nbfm")}
        title={t("Narrow-band FM (VHF voice; key f)")}
      >
        FM
      </button>
      <button
        class={s?.mode === "iq" ? "on" : ""} aria-pressed={!!(s?.mode === "iq")}
        onClick={() => setMode("iq")}
        title={t("I/Q: the channel itself, I left and Q right, around the dial (±5 kHz, ±10 kHz at 24 kS/s): for Rec (an I/Q WAV for SDR programs) or the audio spectrum. Sent as PCM. Key i")}
      >
        IQ
      </button>
    </div>
  );
}

export function Bandwidth() {
  const s = st.value;
  if (!s) return null;
  const presets = bandwidths(s.mode, s.audio_rate);
  const cur = presets.find((b) => s.lo === b.lo && s.hi === b.hi);
  return (
    <div class="group">
      <Drop
        cls="drop-bw"
        label={cur?.label ?? `${((s.hi - s.lo) / 1000).toFixed(1)}k`}
        title={t("Passband: {lo} to {hi} Hz around the dial", { lo: s.lo, hi: s.hi })}
        items={presets.map((b) => ({ key: b.label, label: b.label, on: b === cur, pick: () => setPassband(b.lo, b.hi) }))}
      />
      <CodecCtl drop />
      {audioRates.value.includes(24000) && (
        <Drop
          label={`${s.audio_rate / 1000} kS/s`}
          title={t("Audio rate: 12 kS/s (passband to ±5 kHz) or 24 kS/s (to ±10 kHz: broadcast AM, NBFM, I/Q; twice the data)")}
          items={[12000, 24000].map((r) => ({ key: String(r), label: `${r / 1000} kS/s`, on: s.audio_rate === r, pick: () => setAudio({ audio_rate: r }) }))}
        />
      )}
    </div>
  );
}

const CODECS: { pref: CodecPref; label: string; title: string }[] = [
  {
    pref: "auto",
    label: "Opus",
    title: N_("Opus: 24-48 kbit/s, for slow or metered connections. While a decoder reads the audio, μ-law instead (Opus would cost it weak signals)"),
  },
  { pref: "mulaw", label: "μ-law", title: N_("μ-law: 96 kbit/s, as good as PCM for listening and for the decoders") },
  { pref: "pcm16", label: "PCM", title: N_("PCM: 192 kbit/s, 16 bits, exactly as the receiver made it") },
];

/** How the audio comes: Opus (μ-law for the decoders), μ-law or PCM
 * (the data saver is in Config). `drop`: the codec as a drop-down. */
export function CodecCtl({ drop = false }: { drop?: boolean }) {
  const sc = serverCodecs.value;
  if (!sc.includes("mulaw") && !sc.includes("opus")) return null;
  const pref = codecPref.value;
  const now = codecNow.value;
  const shown = CODECS.filter((c) => c.pref !== "auto" || (sc.includes("opus") && opusDec.value));
  const title =
    pref === "auto" && now !== "opus"
      ? now === "pcm16"
        ? t("Opus is paused: I/Q comes as PCM (Opus and μ-law would bend I and Q apart)")
        : t("Opus is paused: a decoder reads the audio (μ-law until it closes)")
      : t("How the audio comes");
  const label = (c: (typeof CODECS)[number]) => (c.pref === "auto" && pref === "auto" && now !== "opus" ? `Opus (${now === "pcm16" ? "PCM" : "μ-law"})` : c.label);
  const cur = shown.find((c) => c.pref === pref) ?? shown[0];
  return (
    <>
      {drop ? (
        <Drop cls="drop-codec" label={label(cur)} title={t("{what}. {codec}", { what: title, codec: t(cur.title) })} items={shown.map((c) => ({ key: c.pref, label: label(c), title: t(c.title), on: pref === c.pref, pick: () => setCodecPref(c.pref) }))} />
      ) : (
        <span class="seg" title={title}>
          {shown.map((c) => (
            <button key={c.pref} class={pref === c.pref ? "on" : ""} aria-pressed={!!(pref === c.pref)} title={t(c.title)} onClick={() => setCodecPref(c.pref)}>
              {label(c)}
            </button>
          ))}
        </span>
      )}
    </>
  );
}

export function AudioCtl() {
  const s = st.value;
  if (!s) return null;
  return (
    <div class="group">
      <button class={s.agc ? "on" : ""} aria-pressed={!!(s.agc)} onClick={() => setAgc(!s.agc)} title={t("Automatic gain control (AM: follows the carrier)")}>
        AGC
      </button>
      {s.agc && (
        <span class="seg" title={isAm(s.mode) ? t("AGC recovery after the carrier fades: fast follows it at once (the noise rises in each fade), medium and slow hold the gain through quick fades; custom: your own hang and decay") : t("AGC recovery: fast, medium, slow, or custom (your own hang time and decay)")}>
          {(["fast", "medium", "slow", ...(s.agc_hang_ms !== undefined ? ["custom"] : [])] as AgcSpeed[]).map((v) => (
            <button key={v} class={s.agc_speed === v ? "on" : ""} aria-pressed={!!(s.agc_speed === v)} onClick={() => setAudio({ agc_speed: v })}>
              {v[0].toUpperCase()}
            </button>
          ))}
        </span>
      )}
      {s.agc && s.agc_speed === "custom" && s.agc_hang_ms !== undefined && s.agc_decay_db_s !== undefined && (
        <>
          <label title={t("Hang: how long the gain is held after the signal drops")}>
            {/* i18n: AGC hang = how long the gain is held after the signal drops */}
            {tc("AGC", "hang")}
            <input
              type="range"
              min={0}
              max={2000}
              step={50}
              value={s.agc_hang_ms}
              aria-valuetext={`${(s.agc_hang_ms / 1000).toFixed(2)} s`}
              onInput={(e) => setAudio({ agc_hang_ms: Number(e.currentTarget.value) })}
            />
            <span class="num">{(s.agc_hang_ms / 1000).toFixed(2)} s</span>
          </label>
          <label title={t("Decay: how fast the gain comes back after the hang (dB a second)")}>
            {/* i18n: AGC decay = how fast the gain comes back after the hang time */}
            {tc("AGC", "decay")}
            <input
              type="range"
              min={0}
              max={1}
              step={0.01}
              // log scale, 2-100 dB/s
              value={Math.log(s.agc_decay_db_s / 2) / Math.log(50)}
              aria-valuetext={`${s.agc_decay_db_s.toFixed(0)} dB/s`}
              onInput={(e) => setAudio({ agc_decay_db_s: Math.round(2 * 50 ** Number(e.currentTarget.value)) })}
            />
            <span class="num">{s.agc_decay_db_s.toFixed(0)} dB/s</span>
          </label>
        </>
      )}
      <label title={s.agc ? t("Maximum AGC gain") : t("Fixed gain")}>
        {/* i18n: slider label: maximum gain of the AGC, or the fixed gain when AGC is off */}
        {s.agc ? tc("AGC", "max") : tc("AGC", "gain")}
        <input
          type="range"
          min={0}
          max={100}
          value={s.gain_db}
          aria-valuetext={`${s.gain_db.toFixed(0)} dB`}
          onInput={(e) => setAgc(s.agc, Number(e.currentTarget.value))}
        />
        <span class="num">{s.gain_db.toFixed(0)} dB</span>
      </label>
      {!isAm(s.mode) && s.mode !== "iq" && (
        <button
          class={stereo.value ? "on" : ""} aria-pressed={!!(stereo.value)}
          onClick={() => setStereo(!stereo.value)}
          title={t("Pseudo-stereo (headphones): signals placed by pitch, lower to the left and higher to the right (in SSB around the passband's middle pitch); LSB is mirrored to match the waterfall")}
        >
          {/* i18n: button: pseudo-stereo, signals spread left and right by their pitch */}
          {tc("pseudo-stereo", "Stereo")}
        </button>
      )}
      <button class={muted.value ? "on" : ""} aria-pressed={!!(muted.value)} onClick={() => setVolume(volume.value, !muted.value)} title={t("Mute")}>
        {/* i18n: Vol = Volume (short, on a button); Muted = the sound is muted */}
        {muted.value ? t("Muted") : tc("volume", "Vol")}
      </button>
      <input
        type="range"
        min={0}
        max={1}
        step={0.01}
        value={volume.value}
        aria-label={t("Volume")}
        aria-valuetext={`${Math.round(volume.value * 100)} %`}
        onInput={(e) => setVolume(Number(e.currentTarget.value), false)}
        title={t("Volume")}
      />
    </div>
  );
}

/** Last levels of the noise blanker and noise reduction, for turning
 * them back on. */
const nbLevel = { v: 8 };
const nrLevel = { v: 0.6 };

/** The neural voice noise reduction in the page (USB/LSB at 12 kHz). */
function NnCtl() {
  const on = nnOn.value;
  const ok = nnApplies.value;
  const cw = nnKind.value === "cw";
  const choice = nnChoice.value;
  const models = nnKindModels.value;
  return (
    <>
      <button
        class={on && ok ? "on" : ""} aria-pressed={!!on}
        disabled={!ok}
        onClick={() => (nnOn.value = !on)}
        title={!ok
          ? t("Neural noise reduction: for USB, LSB and CW with 12 kHz audio")
          : cw
          ? t("Neural noise reduction for CW, in this browser: keeps the keyed tones, takes away the noise between and around them, never adds sound")
          : t("Neural noise reduction for SSB voice, in this browser: takes away what it hears as noise, never adds sound")}
      >
        {/* i18n: NN = neural (network) noise reduction, a short button label */}
        {tc("filter", "NN")}
      </button>
      {on && ok && (
        <input
          type="range"
          min={0.3}
          max={3}
          step={0.05}
          value={nnStrength.value}
          aria-label={t("Neural noise reduction strength")}
          aria-valuetext={`${(nnStrength.value * 100).toFixed(0)} %`}
          onInput={(e) => (nnStrength.value = Number(e.currentTarget.value))}
          title={t("Neural noise reduction strength {n} %", { n: (nnStrength.value * 100).toFixed(0) })}
        />
      )}
      {on && ok && models.length > 0 && (
        <select
          value={choice.value}
          aria-label={t("Neural noise reduction model")}
          title={t("Neural noise reduction model (for comparing)")}
          onChange={(e) => (choice.value = e.currentTarget.value)}
        >
          <option value="">{t("built-in")}</option>
          {models.map((m) => (
            <option key={m.name} value={m.name} title={m.note}>{m.name}</option>
          ))}
        </select>
      )}
    </>
  );
}

/** Noise blanker, noise reduction, squelch. */
export function DspCtl() {
  const s = st.value;
  if (!s) return null;
  if (s.nb > 0) nbLevel.v = s.nb;
  if (s.nr > 0) nrLevel.v = s.nr;
  return (
    <div class="group">
      <button
        class={s.nb > 0 ? "on" : ""} aria-pressed={!!(s.nb > 0)}
        onClick={() => setAudio({ nb: s.nb > 0 ? 0 : nbLevel.v })}
        title={t("Noise blanker: removes short impulses (ignition, switching noise)")}
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
          aria-label={t("Blanker sensitivity")}
          aria-valuetext={t("impulses over {n} x the average", { n: s.nb.toFixed(1) })}
          onInput={(e) => setAudio({ nb: 33 - Number(e.currentTarget.value) })}
          title={t("Blanker sensitivity: impulses over {n} x the average", { n: s.nb.toFixed(1) })}
        />
      )}
      <button
        class={s.nr > 0 ? "on" : ""} aria-pressed={!!(s.nr > 0)}
        onClick={() => setAudio({ nr: s.nr > 0 ? 0 : nrLevel.v })}
        title={t("Noise reduction: lowers steady background noise")}
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
          aria-label={t("Noise reduction strength")}
          aria-valuetext={`${(s.nr * 100).toFixed(0)} %`}
          onInput={(e) => setAudio({ nr: Number(e.currentTarget.value) })}
          title={t("Noise reduction strength {n} %", { n: (s.nr * 100).toFixed(0) })}
        />
      )}
      <NnCtl />
      <button
        class={s.notch ? "on" : ""} aria-pressed={!!(s.notch)}
        onClick={() => setAudio({ notch: !s.notch })}
        title={t("Automatic notch: removes steady carriers and whistles (heterodynes), leaves speech and CW")}
      >
        {/* i18n: Notch = automatic notch filter removing steady carriers and whistles */}
        {tc("filter", "Notch")}
      </button>
      <button
        class={s.squelch ? "on" : ""} aria-pressed={!!(s.squelch)}
        onClick={() => setAudio({ squelch: !s.squelch })}
        title={t("Squelch: mute while the signal is below the threshold (marked on the S-meter)")}
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
            aria-label={t("Squelch threshold")}
            aria-valuetext={`${s.squelch_db.toFixed(0)} dB`}
            onInput={(e) => setAudio({ squelch_db: Number(e.currentTarget.value) })}
            title={t("Squelch threshold")}
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
      <button onClick={() => zoom(s.wf.decim / 2)} disabled={s.wf.decim <= r.wf_decim_min} title={t("Zoom out (wheel)")} aria-label={t("Zoom out (wheel)")}>
        −
      </button>
      <span class="num" title={t("Waterfall span")}>
        {span >= 1e6 ? `${(span / 1e6).toFixed(2)} MHz` : `${(span / 1e3).toFixed(1)} kHz`}
      </span>
      <button onClick={() => zoom(s.wf.decim * 2)} disabled={s.wf.decim >= r.wf_decim_max} title={t("Zoom in (wheel)")} aria-label={t("Zoom in (wheel)")}>
        +
      </button>
      <button onClick={() => setView({ ...s.wf, center_hz: s.hz })} title={t("Centre the waterfall on the dial")}>
        {/* i18n: button: centre the waterfall on the dial frequency */}
        {tc("waterfall zoom", "Centre")}
      </button>
      <button onClick={full} title={t("Whole range")}>
        {/* i18n: button: zoom the waterfall out to the receiver's whole range */}
        {tc("waterfall zoom", "Full")}
      </button>
    </div>
  );
}

/** The button for the list of users and skimmers (the user's own name
 * is set in Config). */
export function UsersCtl() {
  const s = st.value;
  const open = usersOpen.value;
  const box = useRef<HTMLDivElement>(null);
  // a click elsewhere closes the list
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) usersOpen.value = false;
    };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  }, [open]);
  if (!s) return null;
  const others = users.value;
  return (
    <div class="group users-ctl" ref={box}>
      <button class={open ? "on" : ""} aria-expanded={open} aria-haspopup="dialog" onClick={() => (usersOpen.value = !open)} title={t("Users and background decoders")}>
        {t("Users {n}", { n: others.length + 1 })}
      </button>
      {open && <UserList />}
    </div>
  );
}

function UserList() {
  const s = st.value!;
  const own = lease.value?.granted;
  const pick = (f: () => void) => () => {
    f();
    usersOpen.value = false;
  };
  const pop = usePopFocus(true);
  return (
    <div class="users-pop" ref={pop}>
      <table>
        <thead>
          <tr>
            <th>{t("user")}</th>
            <th class="r">kHz</th>
            <th>{t("mode")}</th>
          </tr>
        </thead>
        <tbody>
          <tr class="me">
            <td>{t("{name} (you)", { name: myName.value || t("user {id}", { id: sessionId.value ?? "?" }) })}</td>
            <td class="r">{formatKhz(s.hz)}</td>
            <td>
              {s.mode.toUpperCase()}
              {!s.share && <span class="dim"> · {t("private")}</span>}
            </td>
          </tr>
          {users.value.map((u) => {
            const f = followable(u);
            return (
              <tr
                key={u.id}
                class={f ? "pick" : "dim"}
                {...rowAction(f ? pick(() => pickUser(u)) : undefined)}
                title={f ? (own ? t("Click to tune there") : t("Click to listen along")) : u.hz === undefined ? t("Their frequency is private") : t("Waiting for a receiver")}
              >
                <td>{u.name || t("user {id}", { id: u.id })}</td>
                <td class="r">{u.hz === undefined ? t("private") : formatKhz(u.hz)}</td>
                <td>
                  {u.mode?.toUpperCase() ?? ""}
                  {!u.rx && ` · ${t("waiting")}`}
                </td>
              </tr>
            );
          })}
        </tbody>
        {skimmers.value.length > 0 && (
          <>
            <thead>
              <tr>
                <th colSpan={3}>{t("background decoders")}</th>
              </tr>
            </thead>
            <tbody>
              {skimmers.value.map((k) => (
                <tr key={`${k.mode}${k.dial}`} class={k.on ? "pick" : "pick dim"} {...rowAction(pick(() => pickSkimmer(k)))} title={t("Click to tune there")}>
                  <td>{k.mode}</td>
                  <td class="r">{formatKhz(k.dial)}</td>
                  <td>USB{!k.on && ` · ${t("waiting")}`}</td>
                </tr>
              ))}
            </tbody>
          </>
        )}
      </table>
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
        <span>{t("Your receiver is ready.")}</span>
        <button onClick={() => listenTo(null)}>{t("Switch to it")}</button>
      </>
    ) : (
      <>
        <span>{t("Listening along with user {id} (waiting for a receiver of your own, #{pos} in the queue).", { id: s.listen, pos: l?.position ?? "?" })}</span>
        <button onClick={() => listenTo(null)}>{t("Stop")}</button>
      </>
    );
  } else if (l && !l.granted) {
    const g = guestOpen.value;
    body = (
      <span>
        {g > 0
          ? tn(
              "Visitors without the password share one receiver, now in use: you are #{pos} in the queue, waterfall only until then.",
              "Visitors without the password share {n} receivers, all in use: you are #{pos} in the queue, waterfall only until then.",
              g,
              { pos: l.position },
            )
          : t("All receivers are in use: you are #{pos} in the queue, waterfall only until then.", { pos: l.position })}
        {users.value.some(followable) && ` ${t("Click another user's marker (▼) on the scale to listen along.")}`}
        {g > 0 && (
          <>
            {" "}
            <button onClick={guestLogin}>{t("Log in")}</button>
          </>
        )}
      </span>
    );
  }
  if (!body && !n) return null;
  return (
    <div class="banner" role="status">
      {body}
      {n && <span class="warn">{n}</span>}
    </div>
  );
}

export function ContrastCtl() {
  const r = range.value;
  return (
    <div class="group" title={t("Waterfall colour range (dBFS)")}>
      <button class={r.auto ? "on" : ""} aria-pressed={!!(r.auto)} onClick={() => (range.value = { ...r, auto: !r.auto })}>
        {/* i18n: button: choose the waterfall colour range automatically */}
        {tc("colour range", "Auto")}
      </button>
      <label>
        {/* i18n: waterfall colour range slider: floor = the level shown as the darkest colour */}
        {tc("colour range", "floor")}
        <input
          type="range"
          min={-170}
          max={-20}
          value={r.min}
          aria-valuetext={`${r.min} dBFS`}
          onInput={(e) => (range.value = { ...setFloor(r, Number(e.currentTarget.value)), auto: false })}
        />
      </label>
      <label>
        {/* i18n: waterfall colour range slider: top = the level shown as the brightest colour */}
        {tc("colour range", "top")}
        <input
          type="range"
          min={-150}
          max={0}
          value={r.max}
          aria-valuetext={`${r.max} dBFS`}
          onInput={(e) => (range.value = { ...setTop(r, Number(e.currentTarget.value)), auto: false })}
        />
      </label>
      <span class="num">
        {r.min}…{r.max}
      </span>
    </div>
  );
}

/** Peak hold, a snapshot and the audio spectrum (the waterfall's colours
 * and time marks are in Config). */
export function LookCtl() {
  return (
    <div class="group look">
      <button class={peakHold.value ? "on" : ""} aria-pressed={!!(peakHold.value)} onClick={() => setPeakHold(!peakHold.value)} title={t("Peak hold: the highest level each frequency reached (pink), on the spectrum")}>
        {/* i18n: Peak = peak hold: the spectrum keeps the highest level each frequency reached */}
        {tc("spectrum", "Peak")}
      </button>
      {peakHold.value && (
        <button onClick={() => paneHooks.clearPeak?.()} title={t("Clear the peaks")} aria-label={t("Clear the peaks")}>
          ↺
        </button>
      )}
      <button onClick={() => paneHooks.saveImage?.()} title={t("Save the spectrum and waterfall as a PNG image (with time marks and the frequency)")}>
        PNG
      </button>
      <button class={afOn.value ? "on" : ""} aria-pressed={!!(afOn.value)} onClick={() => setAf(!afOn.value)} title={t("Audio spectrum: the audio's spectrum and waterfall in a panel (in IQ, both sides of the dial)")}>
        AF
      </button>
    </div>
  );
}

/** The RF attenuator, while the owner has it in (for every listener). */
export function AttLamp() {
  const db = status.value?.rf_atten_db;
  if (!db) return null;
  return (
    <span class="lamp att" title={t("RF attenuator in: every signal is {db} dB weaker (set by the receiver's owner, against overloads)", { db })}>
      ATT {db}
    </span>
  );
}

export function SMeter() {
  const p = rssi.value;
  const lo = -140;
  const hi = -20;
  const pos = (db: number) => Math.min(1, Math.max(0, (db - lo) / (hi - lo)));
  const lvl = pos(p);
  const over = performance.now() - overloadAt.value < 1000;
  const s = st.value;
  return (
    <div class="smeter" title={t("Channel power before AGC (dBFS)")}>
      <div class="bar">
        <div class="fill" style={{ width: `${(lvl * 100).toFixed(1)}%` }} />
        {s?.squelch && <div class="sql" style={{ left: `${(pos(s.squelch_db) * 100).toFixed(1)}%` }} title={t("Squelch threshold")} />}
      </div>
      <span class="num">{p > -199 ? `${p.toFixed(1)} dBFS` : "—"}</span>
      <span class={over ? "lamp lit" : "lamp"} title={t("ADC overload")}>
        {/* i18n: OVL = overload: the ADC input is too strong */}
        {t("OVL")}
        {over ? "!" : ""}
        <span class="sr-only">{over ? t("overloaded") : t("no overload")}</span>
      </span>
      <AttLamp />
    </div>
  );
}

/** A number in the status bar, in a slot `w` characters wide (right
 * aligned unless `left`, tabular digits): the bar doesn't shift as values change. */
const CONN_TEXT: Record<string, string> = {
  connecting: N_("connecting"),
  open: N_("open"),
  closed: N_("closed"),
  busy: N_("busy"),
  ended: N_("ended"),
};

const N = ({ v, w, left }: { v: string | number; w: number; left?: boolean }) => (
  <span class="sn" style={{ minWidth: `${w}ch`, textAlign: left ? "left" : undefined }}>
    {v}
  </span>
);

export function StatusBar() {
  const c = conn.value;
  const s = status.value;
  const a = audioStats.value;
  const r = radio.value;
  return (
    <div class="status">
      <span class={`conn ${c}`} role="status">{t(CONN_TEXT[c])}</span>
      {connNote.value && <span>{connNote.value}</span>}
      {r && (
        <span>
          {r.backend} · {(r.rx_rate / 1e3).toFixed(0)} kS/s
        </span>
      )}
      {latencyMs.value !== null && (
        <span>
          {/* i18n: rtt = round-trip time to the receiver, in milliseconds */}
          {tx("rtt {v} ms", { v: <N v={latencyMs.value.toFixed(0)} w={4} /> })}
        </span>
      )}
      {audioKbps.value !== null && st.value && (
        <span title={t("How the audio comes, and its data rate")}>
          {tx("{codec} {v} kbit/s", { codec: st.value.codec ?? "pcm16", v: <N v={audioKbps.value.toFixed(0)} w={3} /> })}
        </span>
      )}
      {wfKbps.value !== null && (
        <span title={t("The waterfall's data rate")}>
          {/* i18n: wf = waterfall */}
          {tx("wf {v} kbit/s", { v: <N v={wfKbps.value.toFixed(0)} w={3} /> })}
        </span>
      )}
      {audioOn.value && a && (
        <span title={t("Audio buffer, underruns, playback trim")}>
          {/* i18n: buf = audio buffer; underruns = the buffer ran empty and the sound broke up; ppm = parts per million (playback speed trim) */}
          {tx("buf {fill} ms · {under} underruns · {trim} ppm", {
            fill: <N v={a.fillMs.toFixed(0)} w={4} />,
            under: <N v={a.underruns} w={3} />,
            trim: <N v={((a.trim - 1) * 1e6).toFixed(0)} w={5} />,
          })}
        </span>
      )}
      {audioOn.value && player.fallback && (
        <span title={t("Over plain HTTP the browser allows audio only on the page's own thread, with more buffering. The same page over HTTPS (the unit's own certificate: the browser asks once) plays with less delay.")}>
          {t("http audio (+delay):")}{" "}
          <a href={`https://${location.host}${location.pathname}${location.search}${location.hash}`}>https</a>
        </span>
      )}
      {s && (
        <span title={t("Frames dropped (audio / waterfall), audio gaps")}>
          {tx("drops {audio}/{wf} · gaps {gaps}", {
            audio: <N v={s.audio_drops} w={3} />,
            wf: <N v={s.wf_drops} w={3} left />,
            gaps: <N v={audioGaps.value} w={3} />,
          })}
        </span>
      )}
      {s && !s.hw_rate_ok && <span class="warn">{t("hardware falling behind")}</span>}
      {s && s.clock_ppm !== 0 && (
        <span>
          {tx("clock {v} ppm", { v: <N v={s.clock_ppm.toFixed(3)} w={6} /> })}
        </span>
      )}
      {s && (s.cpu !== undefined || s.temp_c !== undefined) && (
        <span title={t("The receiver's CPU load (all cores) and temperature")}>
          {s.cpu !== undefined && <span class={s.cpu >= 90 ? "warn" : ""}>{tx("cpu {v} %", { v: <N v={s.cpu.toFixed(0)} w={3} /> })}</span>}
          {s.cpu !== undefined && s.temp_c !== undefined && " · "}
          {s.temp_c !== undefined && <span class={s.temp_c >= 85 ? "warn" : ""}><N v={s.temp_c.toFixed(0)} w={3} /> °C</span>}
        </span>
      )}
      <span class="right">
        {guestOpen.value > 0 && (
          <>
            <button
              type="button"
              class="link"
              onClick={guestLogin}
              title={t("You listen as a visitor: {n} of the receivers are open without the password. Log in for any.", { n: guestOpen.value })}
            >
              {t("visitor: log in")}
            </button>
            {" · "}
          </>
        )}
        {server.value} ·{" "}
        <button
          type="button"
          class={infoOpen.value ? "link on" : "link"}
          aria-expanded={infoOpen.value}
          aria-haspopup="dialog"
          onClick={() => (infoOpen.value = !infoOpen.value)}
          title={t("About this receiver: where it is, its antenna, its limits, privacy, licences")}
        >
          {t("info")}
        </button>{" "}
        ·{" "}
        <button type="button" class="link" aria-haspopup="dialog" onClick={() => (helpOpen.value = true)} title={t("Keys and mouse (key ?)")}>
          {t("keys")}
        </button>{" "}
        · <a href="#/admin">{t("admin")}</a>
      </span>
    </div>
  );
}
