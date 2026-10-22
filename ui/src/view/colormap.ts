// SPDX-License-Identifier: GPL-3.0-or-later
// Waterfall palette: dark blue noise floor through cyan and yellow to red
// and white peaks. 256 RGBA entries.

const STOPS: [number, [number, number, number]][] = [
  [0.0, [8, 10, 18]],
  [0.15, [16, 24, 72]],
  [0.35, [24, 80, 170]],
  [0.55, [40, 190, 200]],
  [0.72, [230, 220, 60]],
  [0.87, [235, 90, 30]],
  [1.0, [255, 245, 235]],
];

export function colormap(): Uint8Array {
  const out = new Uint8Array(256 * 4);
  for (let i = 0; i < 256; i++) {
    const t = i / 255;
    let k = 0;
    while (k < STOPS.length - 2 && STOPS[k + 1][0] < t) k++;
    const [t0, c0] = STOPS[k];
    const [t1, c1] = STOPS[k + 1];
    const f = Math.min(1, Math.max(0, (t - t0) / (t1 - t0)));
    for (let j = 0; j < 3; j++) out[4 * i + j] = Math.round(c0[j] + (c1[j] - c0[j]) * f);
    out[4 * i + 3] = 255;
  }
  return out;
}

/** CSS colour of palette position t (0..1), for legends. */
export function cssColor(t: number): string {
  const m = colormap();
  const i = 4 * Math.round(Math.min(1, Math.max(0, t)) * 255);
  return `rgb(${m[i]},${m[i + 1]},${m[i + 2]})`;
}
