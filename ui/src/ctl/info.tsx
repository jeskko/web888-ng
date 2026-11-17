// SPDX-License-Identifier: GPL-3.0-or-later
// The receiver's info page (`/api/info`): its name, place, station and
// antenna, the owner's words and photo, and the limits a listener meets.
// The Info button opens it, as does a link to the page with #info.

import { signal } from "@preact/signals";
import { useEffect, useState } from "preact/hooks";
import { api } from "../state.ts";
import { Licences } from "./licences.tsx";
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

const minutes = (m: number) => (m % 60 === 0 && m >= 60 ? `${m / 60} h` : `${m} min`);

/** The limits, in sentences (none: nothing to say). */
export function rulesText(r: Info["rules"]): string[] {
  const out = [`${r.receivers} receivers${r.password ? (r.open_receivers > 0 ? `, ${r.open_receivers} of them open without the password` : ", with the password") : ""}.`];
  if (r.per_ip > 0) out.push(`At most ${r.per_ip} session${r.per_ip === 1 ? "" : "s"} at a time from one address.`);
  if (r.session_minutes > 0) out.push(`A session lasts at most ${minutes(r.session_minutes)}.`);
  if (r.idle_minutes > 0) out.push(`A session without activity ends after ${minutes(r.idle_minutes)}.`);
  if (r.daily_minutes > 0) out.push(`${minutes(r.daily_minutes)} of listening a day from one address (the day changes at 00:00 UTC).`);
  return out;
}

function Body({ i }: { i: Info }) {
  const station = [i.callsign, i.locator].filter(Boolean).join(" · ");
  return (
    <>
      <h3>{i.name || "web888d receiver"}</h3>
      {i.location && <p class="info-where">{i.location}</p>}
      {i.photo && <img class="info-photo" src={api("api/info/photo")} alt="The receiver's photo" />}
      <dl class="info-facts">
        {station && (
          <>
            <dt>Station</dt>
            <dd>{station}</dd>
          </>
        )}
        {i.antenna && (
          <>
            <dt>Antenna</dt>
            <dd>{i.antenna}</dd>
          </>
        )}
        {i.contact && (
          <>
            <dt>Contact</dt>
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
      <h4>For listeners</h4>
      <ul class="info-rules">
        {rulesText(i.rules).map((t) => (
          <li key={t}>{t}</li>
        ))}
        {i.reception_notice && <li>{RECEPTION_NOTICE}</li>}
      </ul>
      <details class="info-privacy">
        <summary>Privacy</summary>
        {privacyText(i.privacy).map((t) => (
          <p key={t}>{t}</p>
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
      <p class="dim">{i.server}: open-source receiver software (web888-ng).</p>
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
      .catch((e: Error) => setErr(`The info page didn't load (${e.message}).`));
    const key = (e: KeyboardEvent) => e.key === "Escape" && (infoOpen.value = false);
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [open]);
  if (!open) return null;
  return (
    <div class="overlay keyhelp" role="dialog" aria-label="About this receiver" onClick={(e) => e.target === e.currentTarget && (infoOpen.value = false)}>
      <div class="keyhelp-box info-box">
        <button class="info-x" onClick={() => (infoOpen.value = false)} title="Close (Escape)">
          ✕
        </button>
        {!info ? <p class="dim">{err || "Loading…"}</p> : lic ? <Licences server={info.server} /> : <Body i={info} />}
        {info && (
          <p class="info-foot">
            <button onClick={() => setLic(!lic)}>{lic ? "← About this receiver" : "Licences and credits"}</button>
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
      <button class={infoOpen.value ? "on" : ""} onClick={() => (infoOpen.value = !infoOpen.value)} title="About this receiver: where it is, its antenna, its limits">
        Info
      </button>
    </div>
  );
}
