// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { privacyText, RECEPTION_NOTICE, type Privacy } from "../src/privacy.ts";

const base: Privacy = { per_ip: false, daily: false, blocked: false, proxy: false, contact: "", text: "", services: [] };

test("the privacy section follows the settings", () => {
  const plain = privacyText(base).join(" ");
  assert.match(plain, /used while you are connected, to run your session\./);
  assert.match(plain, /None of this is written to disk/);
  assert.match(plain, /talks to no outside services/);
  assert.match(plain, /go to the receiver's owner\./);
  assert.doesNotMatch(plain, /daily limit|blocked|proxy/);

  const all = privacyText({
    ...base,
    per_ip: true,
    daily: true,
    blocked: true,
    proxy: true,
    contact: "owner@example.org",
    services: [
      { name: "NOAA SWPC", what: "sunspot numbers" },
      { name: "GitHub", what: "releases" },
    ],
  }).join(" ");
  assert.match(all, /run your session and to limit the sessions from one address/);
  assert.match(all, /daily limit/);
  assert.match(all, /Apart from blocked addresses, none of this is written to disk/);
  assert.match(all, /proxy server/);
  assert.match(all, /none of which is told anything about you: NOAA SWPC \(sunspot numbers\); GitHub \(releases\)\./);
  assert.match(all, /owner: owner@example\.org\./);
  assert.match(RECEPTION_NOTICE, /must not pass on/);
});
