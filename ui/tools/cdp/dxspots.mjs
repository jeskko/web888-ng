// SPDX-License-Identifier: GPL-3.0-or-later
// Live DX spots as labels, and a typed frequency bringing the waterfall
// along: the DX labels the server has in view (and a screenshot of them),
// then a frequency typed inside the view (the view stays) and one far
// outside it (the view follows).
//   CHROME=... node dxspots.mjs URL OUT_PREFIX
import { launch, q, sleep } from "./lib.mjs";
const [url, out = "/tmp/dxspots"] = process.argv.slice(2);
const p = await launch({ width: 1400, height: 900 });
await p.goto(`${url.replace(/#.*/, "")}#f=7074.0&m=usb&z=256&c=7100.0`);
await sleep(8000);
const view = async () => Object.fromEntries(new URLSearchParams((await p.ev("location.hash")).slice(1)));
const v = await view();
console.log("view", v);
const dx = await p.ev(`fetch("api/markers?lo=6800000&hi=7400000&src=dx").then((r) => r.json()).then((j) => j.markers.map((m) => m.name + " " + m.hz / 1000))`);
console.log(`dx labels near 40 m: ${dx.length}`, dx.slice(0, 8));
console.log("label sources", await p.ev(`localStorage.getItem("web888.labels") ?? "(default: all)"`));
await p.shot(`${out}-labels.png`);

async function typeFreq(text) {
  await p.ev(`${q(".freq")}.click()`);
  await sleep(300);
  await p.type(q(".freq-input"), text);
  for (const type of ["keyDown", "keyUp"]) {
    await p.cdp("Input.dispatchKeyEvent", { type, key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
  }
  await sleep(2500);
  return view();
}
const near = await typeFreq("7120");
console.log("typed 7120 (in view):", near, near.c === v.c ? "view stayed" : "VIEW MOVED");
const far = await typeFreq("14025");
const centre = Number(far.c);
console.log("typed 14025 (off view):", far, Math.abs(centre - 14025) < 50 ? "view followed" : "VIEW DID NOT FOLLOW");
await p.shot(`${out}-typed.png`);
p.close();
