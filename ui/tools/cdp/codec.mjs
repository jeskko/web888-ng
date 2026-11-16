// SPDX-License-Identifier: GPL-3.0-or-later
// The audio's codec: Opus by default, mu-law while a decoder reads the
// audio, mu-law or PCM when chosen; then the data saver. Prints the status
// bar's codec and rates and the player's underruns at each step, and
// saves codec.png in OUT (default .).
//   CHROME=... node codec.mjs URL
import { decoder, launch, sleep } from "./lib.mjs";
const url = process.argv[2];
const out = process.env.OUT ?? ".";
const p = await launch({ autoplay: true });
await p.goto(url);
await sleep(3000);
await p.ev("document.querySelector('.start')?.click()");
const status = async (what) => {
  await sleep(5000);
  const s = await p.ev("document.querySelector('.status').textContent");
  const codec = s.match(/(pcm16|mulaw|opus)\s*(\d+)\s*kbit\/s/);
  const wf = s.match(/wf\s*(\d+)\s*kbit\/s/);
  const under = s.match(/(\d+)\s*underruns/);
  const seg = await p.ev(
    "[...document.querySelectorAll('.seg button')].filter((b) => /Opus|law|PCM/.test(b.textContent)).map((b) => (b.classList.contains('on') ? '[' + b.textContent + ']' : b.textContent)).join(' ')",
  );
  console.log(`${what}: ${codec ? `${codec[1]} ${codec[2]} kbit/s` : "no codec in the status bar"}, wf ${wf?.[1] ?? "?"} kbit/s, underruns ${under?.[1] ?? "?"}; buttons: ${seg}`);
};
const choose = (label) => p.ev(`[...document.querySelectorAll('button')].find((b) => b.textContent.startsWith(${JSON.stringify(label)})).click()`);
await status("start");
await p.shot(`${out}/codec.png`);
await decoder(p, "FT8/FT4/WSPR");
await status("FT8 decoder open");
await decoder(p, "FT8/FT4/WSPR", false);
await status("FT8 decoder closed");
await choose("μ-law");
await status("μ-law chosen");
await choose("PCM");
await status("PCM chosen");
await choose("Opus");
await status("Opus chosen");
await choose("Saver");
await status("data saver on");
await choose("Saver");
await status("data saver off");
console.log("errors:", await p.ev("window.__errors?.length ?? 'n/a'"));
p.close();
