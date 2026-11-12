// SPDX-License-Identifier: GPL-3.0-or-later
import assert from "node:assert/strict";
import { test } from "node:test";
import { bearing, distanceKm, ftxSender, GridBook, gridToLatLon, isCall, latLonToGrid, subsolar, wsprSender } from "../src/geo.ts";

const near = (a: number, b: number, eps: number) => assert.ok(Math.abs(a - b) <= eps, `${a} vs ${b}`);

test("locators to places and back", () => {
  // JN58td: Munich area
  const [lat, lon] = gridToLatLon("JN58td")!;
  near(lat, 48.146, 0.01);
  near(lon, 11.625, 0.01);
  assert.equal(latLonToGrid([lat, lon], 6), "JN58td");
  assert.deepEqual(gridToLatLon("FN42"), [42.5, -71]);
  assert.deepEqual(gridToLatLon("AA"), [-85, -170]);
  assert.equal(latLonToGrid([42.5, -71], 4), "FN42");
  assert.equal(latLonToGrid([-90, -180], 8), "AA00aa00");
  assert.equal(latLonToGrid([90, 180], 6), "RR99xx");
  assert.equal(gridToLatLon("ZZ00"), null);
  assert.deepEqual(gridToLatLon("RR73"), [83.5, 175]); // a square (so FT8's RR73 is no locator)
  assert.equal(gridToLatLon("FN4"), null);
});

test("great circle distance and bearing", () => {
  const helsinki: [number, number] = [60.17, 24.94];
  const newYork: [number, number] = [40.71, -74.01];
  near(distanceKm(helsinki, newYork), 6620, 30);
  near(bearing(helsinki, newYork), 299.7, 0.5);
  near(bearing([51.5, -0.13], newYork), 288.3, 0.5);
  near(bearing([0, 0], [10, 0]), 0, 1e-9);
  near(bearing([0, 0], [0, 10]), 90, 1e-9);
  near(distanceKm([0, 0], [0, 180]), Math.PI * 6371, 1);
});

test("the sun overhead", () => {
  // 2026-06-21 12:00 UTC: near the Tropic of Cancer, near Greenwich
  const [lat, lon] = subsolar(Date.UTC(2026, 5, 21, 12) / 1000);
  near(lat, 23.44, 0.2);
  near(lon, 0, 1);
  // 2026-12-21 00:00 UTC: Tropic of Capricorn, the date line
  const [lat2, lon2] = subsolar(Date.UTC(2026, 11, 21, 0) / 1000);
  near(lat2, -23.44, 0.2);
  near(Math.abs(lon2), 180, 2);
});

test("senders of FT8/FT4 messages", () => {
  assert.deepEqual(ftxSender("CQ K1ABC FN42"), { call: "K1ABC", grid: "FN42" });
  assert.deepEqual(ftxSender("CQ DX K1ABC FN42"), { call: "K1ABC", grid: "FN42" });
  assert.deepEqual(ftxSender("CQ POTA OH2XYZ KP20"), { call: "OH2XYZ", grid: "KP20" });
  assert.deepEqual(ftxSender("OH2XYZ K1ABC FN42"), { call: "K1ABC", grid: "FN42" });
  assert.deepEqual(ftxSender("OH2XYZ K1ABC -12"), { call: "K1ABC", grid: undefined });
  assert.deepEqual(ftxSender("OH2XYZ K1ABC RR73"), { call: "K1ABC", grid: undefined });
  assert.deepEqual(ftxSender("OH2XYZ <K1ABC/P> R-05"), { call: "K1ABC/P", grid: undefined });
  assert.deepEqual(ftxSender("OH2XYZ <...> 73"), {});
  assert.deepEqual(ftxSender("TNX 73 GL"), {});
  assert.ok(isCall("3DA0XYZ") && isCall("VP2MAA") && isCall("OH8X/P"));
  assert.ok(!isCall("CQ") && !isCall("DX") && !isCall("123"));
});

test("senders of WSPR messages", () => {
  assert.deepEqual(wsprSender("K1ABC FN42 37"), { call: "K1ABC", grid: "FN42" });
  assert.deepEqual(wsprSender("<PJ4/K1ABC> FK52UD 37"), { call: "PJ4/K1ABC", grid: "FK52UD" });
  assert.deepEqual(wsprSender("X Y"), {});
});

test("the last locator of each call", () => {
  const b = new GridBook(2);
  b.note("A1A", "FN42");
  b.note("B2B", "KP20");
  b.note("A1A", undefined);
  assert.equal(b.get("A1A"), "FN42");
  b.note("C3C", "JO65"); // the oldest goes
  assert.equal(b.get("A1A"), undefined);
  assert.equal(b.size, 2);
});
