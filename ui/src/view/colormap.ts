// SPDX-License-Identifier: GPL-3.0-or-later
// Waterfall palettes, each 256 RGBA entries from a few colour stops.
// "web888" (the default): dark blue noise floor through cyan and yellow
// to red and white peaks. The others: viridis (perceptually even, reads
// in grey and for most colour-blind eyes), heat (black, red, yellow,
// white), phosphor (black through green to white, an old scope) and grey.

type Stops = [number, [number, number, number]][];

export const PALETTES = {
  web888: [
    [0.0, [8, 10, 18]],
    [0.15, [16, 24, 72]],
    [0.35, [24, 80, 170]],
    [0.55, [40, 190, 200]],
    [0.72, [230, 220, 60]],
    [0.87, [235, 90, 30]],
    [1.0, [255, 245, 235]],
  ],
  // viridis at nine evenly spaced points
  viridis: [
    [0.0, [68, 1, 84]],
    [0.125, [71, 44, 122]],
    [0.25, [59, 81, 139]],
    [0.375, [44, 113, 142]],
    [0.5, [33, 144, 141]],
    [0.625, [39, 173, 129]],
    [0.75, [92, 200, 99]],
    [0.875, [170, 220, 50]],
    [1.0, [253, 231, 37]],
  ],
  heat: [
    [0.0, [0, 0, 0]],
    [0.3, [110, 0, 0]],
    [0.55, [220, 50, 0]],
    [0.8, [255, 200, 0]],
    [1.0, [255, 255, 240]],
  ],
  phosphor: [
    [0.0, [0, 6, 2]],
    [0.4, [0, 90, 30]],
    [0.75, [60, 230, 90]],
    [1.0, [235, 255, 235]],
  ],
  grey: [
    [0.0, [0, 0, 0]],
    [1.0, [255, 255, 255]],
  ],
} satisfies Record<string, Stops>;

export type Palette = keyof typeof PALETTES;
export const PALETTE_NAMES = Object.keys(PALETTES) as Palette[];

export function isPalette(s: string | null): s is Palette {
  return s !== null && s in PALETTES;
}

const cache = new Map<Palette, Uint8Array>();

/** The palette as 256 RGBA entries. */
export function colormap(name: Palette = "web888"): Uint8Array {
  const hit = cache.get(name);
  if (hit) return hit;
  const stops: Stops = PALETTES[name];
  const out = new Uint8Array(256 * 4);
  for (let i = 0; i < 256; i++) {
    const t = i / 255;
    let k = 0;
    while (k < stops.length - 2 && stops[k + 1][0] < t) k++;
    const [t0, c0] = stops[k];
    const [t1, c1] = stops[k + 1];
    const f = Math.min(1, Math.max(0, (t - t0) / (t1 - t0)));
    for (let j = 0; j < 3; j++) out[4 * i + j] = Math.round(c0[j] + (c1[j] - c0[j]) * f);
    out[4 * i + 3] = 255;
  }
  cache.set(name, out);
  return out;
}

/** CSS colour of palette position t (0..1), for legends and swatches. */
export function cssColor(t: number, name: Palette = "web888"): string {
  const m = colormap(name);
  const i = 4 * Math.round(Math.min(1, Math.max(0, t)) * 255);
  return `rgb(${m[i]},${m[i + 1]},${m[i + 2]})`;
}

/** A CSS gradient of the palette (the picker's swatch). */
export function cssGradient(name: Palette): string {
  const n = 8;
  const parts = Array.from({ length: n + 1 }, (_, i) => cssColor(i / n, name));
  return `linear-gradient(90deg, ${parts.join(", ")})`;
}
