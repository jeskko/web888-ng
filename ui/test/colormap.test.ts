// SPDX-License-Identifier: GPL-3.0-or-later
// Waterfall palettes: lighter at every step (stronger signals never look
// darker than weaker ones), also as seen with colour blindness.
import { test } from "node:test";
import assert from "node:assert/strict";
import { colormap, PALETTE_NAMES } from "../src/view/colormap.ts";

const lin = (v: number) => ((v /= 255) <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
// Machado et al. 2009, severity 1
const CVD: Record<string, number[][]> = {
  normal: [[1, 0, 0], [0, 1, 0], [0, 0, 1]],
  protan: [[0.152286, 1.052583, -0.204868], [0.114503, 0.786281, 0.099216], [-0.003882, -0.048116, 1.051998]],
  deutan: [[0.367322, 0.860646, -0.227968], [0.280085, 0.672501, 0.047413], [-0.01182, 0.04294, 0.968881]],
  tritan: [[1.255528, -0.076749, -0.178779], [-0.078411, 0.930809, 0.147602], [0.004733, 0.691367, 0.3039]],
};
/** CIE L* of an sRGB colour as seen with `kind` of vision. */
function lightness(r: number, g: number, b: number, kind: string): number {
  const l = [lin(r), lin(g), lin(b)];
  const m = CVD[kind];
  const s = m.map((row) => Math.min(1, Math.max(0, row[0] * l[0] + row[1] * l[1] + row[2] * l[2])));
  const y = 0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2];
  return y > 0.008856 ? 116 * Math.cbrt(y) - 16 : 903.3 * y;
}

for (const name of PALETTE_NAMES) {
  test(`palette ${name} gets lighter at every step, with any colour vision`, () => {
    const map = colormap(name);
    for (const kind of Object.keys(CVD)) {
      let prev = -1;
      for (let i = 0; i < 256; i += 8) {
        const L = lightness(map[4 * i], map[4 * i + 1], map[4 * i + 2], kind);
        assert.ok(L >= prev - 0.5, `${name}, ${kind}: L* ${L.toFixed(1)} after ${prev.toFixed(1)} at ${i}`);
        prev = L;
      }
    }
  });
}
