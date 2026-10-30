// SPDX-License-Identifier: GPL-3.0-or-later
// Bundles the UI into dist/: app.js (Preact app), worklet.js (audio
// worklet, loaded separately by the browser), jtty-worker.js (the JTTY
// decoder's worker) with jtty.wasm (from scripts/build-wasm.sh, in wasm/),
// index.html, style.css.
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
      try {
        await access("wasm/jtty.wasm");
        await copyFile("wasm/jtty.wasm", `${out}/jtty.wasm`);
      } catch {
        console.warn("no wasm/jtty.wasm (scripts/build-wasm.sh): JTTY decoding won't load");
      }
    });
  },
};

const options = {
  entryPoints: { app: "src/main.tsx", worklet: "src/audio/worklet.ts", "jtty-worker": "src/jtty/worker.ts" },
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
