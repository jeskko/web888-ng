// SPDX-License-Identifier: GPL-3.0-or-later
// Amateur bands (IARU region 1 edges, Hz) for the phone's band picker
// (pure, unit-tested).

import type { Mode } from "./proto.ts";

export interface Band {
  name: string;
  lo: number;
  hi: number;
  /** Where the band picker lands the first time, and its mode. */
  hz: number;
  mode: Mode;
}

export const BANDS: Band[] = [
  { name: "160", lo: 1_810_000, hi: 2_000_000, hz: 1_850_000, mode: "lsb" },
  { name: "80", lo: 3_500_000, hi: 3_800_000, hz: 3_700_000, mode: "lsb" },
  { name: "60", lo: 5_351_500, hi: 5_366_500, hz: 5_357_000, mode: "usb" },
  { name: "40", lo: 7_000_000, hi: 7_200_000, hz: 7_100_000, mode: "lsb" },
  { name: "30", lo: 10_100_000, hi: 10_150_000, hz: 10_130_000, mode: "usb" },
  { name: "20", lo: 14_000_000, hi: 14_350_000, hz: 14_200_000, mode: "usb" },
  { name: "17", lo: 18_068_000, hi: 18_168_000, hz: 18_130_000, mode: "usb" },
  { name: "15", lo: 21_000_000, hi: 21_450_000, hz: 21_300_000, mode: "usb" },
  { name: "12", lo: 24_890_000, hi: 24_990_000, hz: 24_950_000, mode: "usb" },
  { name: "10", lo: 28_000_000, hi: 29_700_000, hz: 28_400_000, mode: "usb" },
  { name: "6", lo: 50_000_000, hi: 52_000_000, hz: 50_150_000, mode: "usb" },
];

/** The band containing `hz`, if any. */
export function bandOf(hz: number): Band | null {
  return BANDS.find((b) => hz >= b.lo && hz <= b.hi) ?? null;
}
