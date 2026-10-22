// SPDX-License-Identifier: GPL-3.0-or-later
// Headless Chromium over the DevTools protocol, for checking the web UI
// against a running web888d. Always muted (audio would otherwise play on
// the machine's speakers). See README.md.
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Start Chromium (`CHROME`, or the first argument) and attach to its page. */
export async function launch({ width = 1400, height = 900, dpr = 1, autoplay = false } = {}) {
  const chrome = process.env.CHROME;
  if (!chrome) throw new Error("set CHROME to a Chromium binary (e.g. Playwright's cached chrome)");
  const port = Number(process.env.CDP_PORT ?? 9333);
  const args = [
    "--headless=new",
    "--no-sandbox",
    `--remote-debugging-port=${port}`,
    "--use-angle=swiftshader",
    "--enable-unsafe-swiftshader",
    "--mute-audio",
    "--ignore-certificate-errors",
    `--window-size=${width},${height}`,
  ];
  if (autoplay) args.push("--autoplay-policy=no-user-gesture-required");
  const proc = spawn(chrome, [...args, "about:blank"], { stdio: "ignore" });
  let list;
  for (let i = 0; i < 50 && !list; i++) {
    try {
      list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    } catch {
      await sleep(200);
    }
  }
  if (!list) throw new Error(`no DevTools on port ${port}`);
  const ws = new WebSocket(list.find((t) => t.type === "page").webSocketDebuggerUrl);
  await new Promise((r) => (ws.onopen = r));
  let id = 0;
  const pending = new Map();
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.id && pending.has(m.id)) {
      pending.get(m.id)(m);
      pending.delete(m.id);
    } else if (m.method === "Runtime.exceptionThrown") {
      console.log("EXCEPTION:", m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text);
    } else if (m.method === "Runtime.consoleAPICalled" && ["error", "warning"].includes(m.params.type)) {
      console.log(`console.${m.params.type}:`, m.params.args.map((a) => a.value ?? a.description).join(" "));
    }
  };
  const cdp = (method, params = {}) =>
    new Promise((r) => {
      const i = ++id;
      pending.set(i, r);
      ws.send(JSON.stringify({ id: i, method, params }));
    });
  await cdp("Runtime.enable");
  await cdp("Page.enable");
  await cdp("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: dpr, mobile: false });
  const page = {
    cdp,
    /** Evaluate a JS expression in the page (awaits promises). */
    ev: async (expr) =>
      (await cdp("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true })).result?.result?.value,
    goto: (url) => cdp("Page.navigate", { url }),
    shot: async (path) => {
      const r = await cdp("Page.captureScreenshot", { format: "png" });
      writeFileSync(path, Buffer.from(r.result.data, "base64"));
    },
    mouse: (type, x, y, extra = {}) =>
      cdp("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1, ...extra }),
    /** Focus (and select) the element `expr` evaluates to, then type `text`. */
    type: async (expr, text) => {
      await page.ev(`(() => { const e = ${expr}; e.focus(); e.select?.(); })()`);
      await cdp("Input.insertText", { text });
    },
    close: () => {
      ws.close();
      proc.kill();
    },
  };
  return page;
}

/** A selector as a JS expression for `type`. */
export const q = (sel) => `document.querySelector(${JSON.stringify(sel)})`;
