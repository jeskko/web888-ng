// SPDX-License-Identifier: GPL-3.0-or-later
import { test } from "node:test";
import assert from "node:assert/strict";
import { applyDgps, DGPS_LINGER, DGPS_TEXTS, dgpsDials, dgpsHealth, latLonText, type DgpsRun } from "../src/dgps.ts";

const rf = (hz: number) => 286_000 + hz;
const run = (ev: DgpsRun["ev"], act: DgpsRun["act"]): DgpsRun => ({ ev, act, now: 0 });
const hdr = { k: "msg" as const, id: 1, hz: 1000, t: 1, sid: 606, z: 60, seq: 0, health: 0, ok: 5, n: 5 };
const act = [{ id: 1, hz: 1000, baud: 100, q: 0.95, w: 0.98, sync: true }];

test("messages merge per beacon: station, satellites, position, text, almanac", () => {
  let s = applyDgps([], run([{ k: "lock", id: 1, hz: 1000, baud: 100 }, { ...hdr, type: 9, gps: [3, 7, 12] }], act), 100, rf);
  s = applyDgps(s, run([{ ...hdr, type: 3, ok: 4, n: 4, pos: [59.98589, 29.12962, 21.4] }, { ...hdr, type: 31, ok: 4, n: 5, glo: [4, 9] }], act), 101, rf);
  s = applyDgps(
    s,
    run(
      [
        { ...hdr, type: 7, alm: [[59.966, 24.384, 250, 293.5, 0, 400, 100], [0, 0, 0, 190, 0, 0, 25]] },
        { ...hdr, type: 16, text: "TEST" },
        { ...hdr, type: 16, text: "TEST" },
      ],
      act,
    ),
    102,
    rf,
  );
  const b = s[0];
  assert.deepEqual([b.rf, b.station, b.health, b.gps, b.glo, b.pos, b.texts], [287_000, 606, 0, [3, 7, 12], [4, 9], [59.98589, 29.12962], ["TEST"]]);
  assert.deepEqual(b.types, { 3: 1, 7: 1, 9: 1, 16: 2, 31: 1 });
  assert.deepEqual([b.msgs, b.complete], [6, 5]);
  // the empty filler entry is left out
  assert.deepEqual(b.alm, [{ pos: [59.966, 24.384], range: 250, khz: 293.5, health: 0, id: 400, baud: 100 }]);
});

test("texts are capped; a beacon gone lingers, then goes; one found again replaces it", () => {
  let s = applyDgps([], run([{ k: "lock", id: 1, hz: 1000, baud: 100 }], act), 0, rf);
  for (let i = 0; i < DGPS_TEXTS + 2; i++) s = applyDgps(s, run([{ ...hdr, type: 16, text: `T${i}` }], act), 1, rf);
  assert.equal(s[0].texts.length, DGPS_TEXTS);
  assert.equal(s[0].texts[DGPS_TEXTS - 1], `T${DGPS_TEXTS + 1}`);
  s = applyDgps(s, run([], []), 2, rf);
  assert.equal(s[0].active, false);
  assert.equal(applyDgps(s, run([], []), 1 + DGPS_LINGER + 1, rf).length, 0);
  const again = applyDgps(s, run([{ k: "lock", id: 2, hz: 1000.4, baud: 100 }], [{ id: 2, hz: 1000.4, baud: 100, q: 0.9, w: 0.9, sync: true }]), 3, rf);
  assert.deepEqual(again.map((x) => x.id), [2]);
});

test("health, dials, positions", () => {
  assert.deepEqual([0, 5, 6, 7].map(dgpsHealth), ["healthy", "healthy", "not monitored", "unhealthy"]);
  const d = dgpsDials();
  assert.equal(d[0] + 300, 283_500);
  assert.ok(d[d.length - 1] + 3000 >= 325_000);
  for (let i = 1; i < d.length; i++) assert.equal(d[i] - d[i - 1], 2_700);
  assert.equal(latLonText([59.966, -24.384]), "59.97° N 24.38° W");
});
