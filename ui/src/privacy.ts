// SPDX-License-Identifier: GPL-3.0-or-later
// The info page's Privacy section and the reception notice: what this
// receiver keeps about listeners, in sentences built from its settings
// (`/api/info` `privacy`), so they stay true (pure, unit-tested).

import { N_, t } from "./i18n.ts";

export interface Privacy {
  per_ip: boolean;
  daily: boolean;
  blocked: boolean;
  /** Behind a reverse proxy (it may keep logs of its own). */
  proxy: boolean;
  contact: string;
  /** The owner's own words. */
  text: string;
  /** Outside services this receiver talks to. */
  services: { name: string; what: string }[];
}

/** What web888d keeps about listeners, a sentence each. */
export function privacyText(p: Privacy): string[] {
  const owner = t("The receiver's owner can see the addresses of current sessions.");
  const out = [
    (p.per_ip
      ? t("Your IP address is used while you are connected, to run your session and to limit the sessions from one address.")
      : t("Your IP address is used while you are connected, to run your session.")) +
      " " +
      owner,
  ];
  if (p.daily) out.push(t("Your listening minutes are counted per address until the day changes (00:00 UTC), to apply the daily limit."));
  if (p.blocked) out.push(t("The owner has blocked some addresses; those are kept in the receiver's settings until removed."));
  out.push(
    t("Failed logins are counted per address for a short while, to slow down password guessing. Logging in sets a cookie that keeps you logged in for 12 hours."),
    t("A name or callsign you enter is shown to the other listeners. Your settings and frequency memories stay in your browser."),
    p.blocked
      ? t("Apart from blocked addresses, none of this is written to disk: when the receiver restarts, it is gone. There are no analytics, no tracking and no advertising.")
      : t("None of this is written to disk: when the receiver restarts, it is gone. There are no analytics, no tracking and no advertising."),
  );
  if (p.proxy) out.push(t("The receiver is reached through a proxy server, which may keep logs of its own."));
  out.push(
    p.services.length === 0
      ? t("The receiver talks to no outside services.")
      : t("The receiver talks to these services, none of which is told anything about you: {list}.", { list: p.services.map((s) => `${s.name} (${s.what})`).join("; ") }),
  );
  out.push(p.contact ? t("Questions about your data go to the owner: {contact}.", { contact: p.contact }) : t("Questions about your data go to the receiver's owner."));
  return out;
}

/** The reception notice (translate where shown: t(RECEPTION_NOTICE)). */
export const RECEPTION_NOTICE = N_(
  "Some of what can be heard here is not meant for the public: aircraft, ships, utility and private stations. In Finland and many other countries you may listen, but you must not pass on or make use of what such messages say.",
);
