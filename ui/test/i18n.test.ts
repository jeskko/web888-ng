// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { pick, pluralRule, t, tc, tn, tx, useCatalogue } from "../src/i18n.ts";

test("plural rules from Plural-Forms", () => {
  const two = pluralRule("(n != 1)");
  assert.deepEqual([0, 1, 2, 5].map(two), [1, 0, 1, 1]);
  // Polish: one, few, many
  const pl = pluralRule("(n==1 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)");
  assert.deepEqual([1, 2, 4, 5, 12, 22, 25].map(pl), [0, 1, 1, 2, 2, 1, 2]);
  assert.deepEqual([0, 1, 2].map(pluralRule("0")), [0, 0, 0]);
});

test("the browser's languages pick a catalogue", () => {
  assert.equal(pick(["fi-FI", "en-US"], ["en", "fi", "sv"]), "fi");
  assert.equal(pick(["sv-SE"], ["en", "fi", "sv"]), "sv");
  assert.equal(pick(["fr-FR", "de"], ["en", "de"]), "de");
  assert.equal(pick(["fr"], ["en", "de"]), "en");
});

test("messages from a catalogue, English without one", () => {
  useCatalogue(null);
  assert.equal(t("Start audio"), "Start audio");
  assert.equal(t("{n} kbit/s", { n: 32 }), "32 kbit/s");
  assert.equal(tn("{n} user", "{n} users", 3), "3 users");
  useCatalogue({
    plural: "n != 1",
    messages: {
      "Start audio": "Käynnistä ääni",
      "{n} kbit/s": "{n} kbit/s",
      "spectrum\u0004Peak": "Huippu",
      "{n} user": ["{n} käyttäjä", "{n} käyttäjää"],
      "Untranslated": "",
      "Tune to {f} or {more}": "Viritä {f} tai {more}",
    },
  });
  assert.equal(t("Start audio"), "Käynnistä ääni");
  assert.equal(tc("spectrum", "Peak"), "Huippu");
  assert.equal(t("Peak"), "Peak");
  assert.equal(tn("{n} user", "{n} users", 1), "1 käyttäjä");
  assert.equal(tn("{n} user", "{n} users", 7), "7 käyttäjää");
  assert.equal(t("Untranslated"), "Untranslated");
  assert.equal(t("Not there"), "Not there");
  const parts = tx("Tune to {f} or {more}", { f: "7074", more: 1 });
  assert.deepEqual(parts, ["Viritä ", "7074", " tai ", 1, ""]);
  useCatalogue(null);
});
