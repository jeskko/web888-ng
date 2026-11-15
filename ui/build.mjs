// SPDX-License-Identifier: GPL-3.0-or-later
// Bundles the UI into dist/: app.js (Preact app), worklet.js (audio
// worklet, loaded separately by the browser), the decoders' workers with
// their WebAssembly (from scripts/build-wasm.sh, in wasm/): jtty-worker.js
// with jtty.wasm (JTTY), digi-worker.js with digi.wasm (FT8/FT4/WSPR),
// cw-worker.js with cw.wasm (CW), rtty-worker.js with fsk.wasm
// (FSK/RTTY), psk-worker.js with psk.wasm (PSK31/63/125), img-worker.js with img.wasm (fax, SSTV) and sigid-worker.js
// with sigid.wasm (the signal identifier); index.html, style.css, and the
// map's world (map/*.json).
// `node build.mjs --watch` rebuilds on change (development).
import * as esbuild from "esbuild";
import { access, copyFile, mkdir } from "node:fs/promises";

const watch = process.argv.includes("--watch");
const out = "dist";
await mkdir(out, { recursive: true });

const copyStatic = {
  name: "copy-static",
  setup(build) {
    build.onEnd(async () => {
      for (const f of ["index.html", "style.css"]) await copyFile(`src/${f}`, `${out}/${f}`);
      // the map's world (Natural Earth, public domain, via world-atlas)
      await mkdir(`${out}/map`, { recursive: true });
      for (const res of ["110m", "50m"]) await copyFile(`node_modules/world-atlas/countries-${res}.json`, `${out}/map/countries-${res}.json`);
      for (const [f, what] of [
        ["jtty.wasm", "JTTY decoding"],
        ["digi.wasm", "FT8/FT4/WSPR decoding"],
        ["cw.wasm", "CW decoding"],
        ["fsk.wasm", "RTTY decoding"],
        ["psk.wasm", "PSK decoding"],
        ["img.wasm", "FAX/SSTV decoding"],
        ["sigid.wasm", "Signal identification"],
      ]) {
        try {
          await access(`wasm/${f}`);
          await copyFile(`wasm/${f}`, `${out}/${f}`);
        } catch {
          console.warn(`no wasm/${f} (scripts/build-wasm.sh): ${what} won't load`);
        }
      }
    });
  },
};

const options = {
  entryPoints: { app: "src/main.tsx", worklet: "src/audio/worklet.ts", "jtty-worker": "src/jtty/worker.ts", "digi-worker": "src/digi/worker.ts", "cw-worker": "src/cw/worker.ts", "rtty-worker": "src/rtty/worker.ts", "psk-worker": "src/psk/worker.ts", "img-worker": "src/img/worker.ts", "sigid-worker": "src/sigid/worker.ts" },
  outdir: out,
  bundle: true,
  format: "esm",
  target: "es2022",
  jsx: "automatic",
  jsxImportSource: "preact",
  minify: !watch,
  sourcemap: watch ? "inline" : false,
  legalComments: "eof",
  logLevel: "info",
  plugins: [copyStatic],
};

if (watch) {
  const ctx = await esbuild.context(options);
  await ctx.watch();
} else {
  await esbuild.build(options);
}
