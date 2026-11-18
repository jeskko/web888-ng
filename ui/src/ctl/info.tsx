// SPDX-License-Identifier: GPL-3.0-or-later
// The receiver's info page (`/api/info`): its name, place, station and
// antenna, the owner's words and photo, and the limits a listener meets.
// The Info button opens it, as does a link to the page with #info.

import { signal } from "@preact/signals";
import { useDialog } from "../a11y.ts";
import { useEffect, useState } from "preact/hooks";
import { api } from "../state.ts";
import { Licences } from "./licences.tsx";
import { N_, t, tn } from "../i18n.ts";
import { privacyText, RECEPTION_NOTICE, type Privacy } from "../privacy.ts";

export const infoOpen = signal(location.hash === "#info");

interface Info {
  name: string;
  location: string;
  callsign: string;
  locator: string;
  antenna: string;
  description: string;
  contact: string;
  photo: boolean;
  server: string;
  privacy: Privacy;
  reception_notice: boolean;
  rules: {
    receivers: number;
    password: boolean;
    open_receivers: number;
    per_ip: number;
    session_minutes: number;
    idle_minutes: number;
    daily_minutes: number;
  };
}

// i18n: h = hours, min = minutes (units)
const minutes = (m: number) => (m % 60 === 0 && m >= 60 ? t("{n} h", { n: m / 60 }) : t("{n} min", { n: m }));

/** The limits, in sentences (none: nothing to say). */
export function rulesText(r: Info["rules"]): string[] {
  const first =
    r.password && r.open_receivers > 0
      ? t("{n} receivers, {open} of them open without the password.", { n: r.receivers, open: r.open_receivers })
      : r.password
        ? t("{n} receivers, with the password.", { n: r.receivers })
        : t("{n} receivers.", { n: r.receivers });
  const out = [first];
  if (r.per_ip > 0) out.push(tn("At most {n} session at a time from one address.", "At most {n} sessions at a time from one address.", r.per_ip));
  if (r.session_minutes > 0) out.push(t("A session lasts at most {time}.", { time: minutes(r.session_minutes) }));
  if (r.idle_minutes > 0) out.push(t("A session without activity ends after {time}.", { time: minutes(r.idle_minutes) }));
  if (r.daily_minutes > 0) out.push(t("{time} of listening a day from one address (the day changes at 00:00 UTC).", { time: minutes(r.daily_minutes) }));
  return out;
}

function Body({ i }: { i: Info }) {
  const station = [i.callsign, i.locator].filter(Boolean).join(" · ");
  return (
    <>
      <h3>{i.name || t("web888d receiver")}</h3>
      {i.location && <p class="info-where">{i.location}</p>}
      {i.photo && <img class="info-photo" src={api("api/info/photo")} alt={t("The receiver's photo")} />}
      <dl class="info-facts">
        {station && (
          <>
            <dt>{t("Station")}</dt>
            <dd>{station}</dd>
          </>
        )}
        {i.antenna && (
          <>
            <dt>{t("Antenna")}</dt>
            <dd>{i.antenna}</dd>
          </>
        )}
        {i.contact && (
          <>
            <dt>{t("Contact")}</dt>
            <dd>{i.contact}</dd>
          </>
        )}
      </dl>
      {i.description
        .split(/\n\s*\n/)
        .filter((p) => p.trim())
        .map((p, k) => (
          <p key={k} class="info-text">
            {p.trim()}
          </p>
        ))}
      <h4>{t("For listeners")}</h4>
      <ul class="info-rules">
        {rulesText(i.rules).map((x) => (
          <li key={x}>{x}</li>
        ))}
        {i.reception_notice && <li>{t(RECEPTION_NOTICE)}</li>}
      </ul>
      <details class="info-privacy">
        <summary>{t("Privacy")}</summary>
        {privacyText(i.privacy).map((x) => (
          <p key={x}>{x}</p>
        ))}
        {i.privacy.text
          .split(/\n\s*\n/)
          .filter((p) => p.trim())
          .map((p, k) => (
            <p key={`o${k}`} class="info-text">
              {p.trim()}
            </p>
          ))}
      </details>
      <p class="dim">{t("{server}: open-source receiver software (web888-ng).", { server: i.server })}</p>
    </>
  );
}

/** The info page as a dialog over the receiver. */
export function InfoPanel() {
  const open = infoOpen.value;
  const [info, setInfo] = useState<Info | null>(null);
  const [err, setErr] = useState("");
  const [lic, setLic] = useState(false);
  useEffect(() => {
    if (!open) return;
    setLic(false);
    fetch(api("api/info"))
      .then((r) => (r.ok ? (r.json() as Promise<Info>) : Promise.reject(new Error(`${r.status}`))))
      .then(setInfo)
      .catch((e: Error) => setErr(t("The info page didn't load ({msg}).", { msg: e.message })));
  }, [open]);
  const box = useDialog(open, () => (infoOpen.value = false));
  if (!open) return null;
  return (
    <div ref={box} class="overlay keyhelp" role="dialog" aria-modal="true" aria-label={t("About this receiver")} onClick={(e) => e.target === e.currentTarget && (infoOpen.value = false)}>
      <div class="keyhelp-box info-box">
        <button class="info-x" onClick={() => (infoOpen.value = false)} title={t("Close (Escape)")} aria-label={t("Close (Escape)")}>
          ✕
        </button>
        {!info ? <p class="dim">{err || t("Loading…")}</p> : lic ? <Licences server={info.server} /> : <Body i={info} />}
        {info && (
          <p class="info-foot">
            <button onClick={() => setLic(!lic)}>{lic ? t("← About this receiver") : t("Licences and credits")}</button>
          </p>
        )}
      </div>
    </div>
  );
}

/** The header's Info button. */
export function InfoCtl() {
  return (
    <div class="group">
      <button class={infoOpen.value ? "on" : ""} aria-expanded={infoOpen.value} aria-haspopup="dialog" onClick={() => (infoOpen.value = !infoOpen.value)} title={t("About this receiver: where it is, its antenna, its limits")}>
        {t("Info")}
      </button>
    </div>
  );
}
