// SPDX-License-Identifier: GPL-3.0-or-later
// Bundles the UI into dist/: app.js (Preact app), worklet.js (audio
// worklet, loaded separately by the browser), the decoders' workers with
// their WebAssembly (from scripts/build-wasm.sh, in wasm/): jtty-worker.js
// with jtty.wasm (JTTY), digi-worker.js with digi.wasm (FT8/FT4/WSPR),
// cw-worker.js with cw.wasm (CW), rtty-worker.js with fsk.wasm
// (FSK/RTTY), psk-worker.js with psk.wasm (PSK31/63/125), dgps-worker.js
// with dgps.wasm (DGPS beacons), hfdl-worker.js
// with hfdl.wasm (HFDL), js8-worker.js with js8.wasm (JS8), img-worker.js with img.wasm (fax, SSTV), sigid-worker.js
// with sigid.wasm (the signal identifier) and timecode-worker.js with
// timecode.wasm (time-code stations); opus.wasm (the audio's Opus
// decoder) and nr.wasm (the neural SSB noise reduction), loaded by the
// page itself; index.html, style.css, and the
// map's world (map/*.json), and the translations (i18n/*.json from po/).
// `node build.mjs --watch` rebuilds on change (development).
import * as esbuild from "esbuild";
import { execFileSync } from "node:child_process";
import { access, copyFile, mkdir } from "node:fs/promises";

const watch = process.argv.includes("--watch");
const out = "dist";
await mkdir(out, { recursive: true });

const copyStatic = {
  name: "copy-static",
  setup(build) {
    build.onEnd(async () => {
      for (const f of ["index.html", "style.css"]) await copyFile(`src/${f}`, `${out}/${f}`);
      // the translations: po/*.po -> dist/i18n/*.json (po/README.md)
      execFileSync(process.execPath, ["scripts/i18n.mjs", "compile", out], { stdio: "inherit" });
      // the map's world (Natural Earth, public domain, via world-atlas)
      await mkdir(`${out}/map`, { recursive: true });
      for (const res of ["110m", "50m"]) await copyFile(`node_modules/world-atlas/countries-${res}.json`, `${out}/map/countries-${res}.json`);
      for (const [f, what] of [
        ["jtty.wasm", "JTTY decoding"],
        ["digi.wasm", "FT8/FT4/WSPR decoding"],
        ["cw.wasm", "CW decoding"],
        ["fsk.wasm", "RTTY decoding"],
        ["psk.wasm", "PSK decoding"],
        ["dgps.wasm", "DGPS decoding"],
        ["hfdl.wasm", "HFDL decoding"],
        ["js8.wasm", "JS8 decoding"],
        ["img.wasm", "FAX/SSTV decoding"],
        ["sigid.wasm", "Signal identification"],
        ["timecode.wasm", "Time-code decoding"],
        ["opus.wasm", "Opus audio (the page falls back to mu-law)"],
        ["nr.wasm", "Neural noise reduction (NN)"],
      ]) {
        try {
          await access(`wasm/${f}`);
          await copyFile(`wasm/${f}`, `${out}/${f}`);
        } catch {
          console.warn(`no wasm/${f} (scripts/build-wasm.sh): ${what} won't load`);
        }
      }
      // the generative voice restorer's weights (fetched when "Gen" is
      // first turned on)
      await copyFile("../crates/nr/model/nrgen.bin", `${out}/nrgen.bin`);
    });
  },
};

const options = {
  entryPoints: { app: "src/main.tsx", worklet: "src/audio/worklet.ts", "jtty-worker": "src/jtty/worker.ts", "digi-worker": "src/digi/worker.ts", "cw-worker": "src/cw/worker.ts", "rtty-worker": "src/rtty/worker.ts", "psk-worker": "src/psk/worker.ts", "dgps-worker": "src/dgps/worker.ts", "hfdl-worker": "src/hfdl/worker.ts", "js8-worker": "src/js8/worker.ts", "img-worker": "src/img/worker.ts", "sigid-worker": "src/sigid/worker.ts", "timecode-worker": "src/timecode/worker.ts" },
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
