// SPDX-License-Identifier: GPL-3.0-or-later
// The info box's Licences view: web888d's own licence and sources, the
// code it was ported from, the data it carries or fetches, and the
// third-party libraries in this build (`/api/info/licences`, written by
// `scripts/third-party.py`).

import { Fragment } from "preact";
import { useEffect, useState } from "preact/hooks";
import { api } from "../state.ts";

interface Lib {
  name: string;
  version: string;
  license: string;
  url: string;
}

interface Libs {
  rust: Lib[];
  npm: Lib[];
}

const A = ({ href, children }: { href: string; children: preact.ComponentChildren }) => (
  <a href={href} target="_blank" rel="noopener noreferrer">
    {children}
  </a>
);

/** Code ported or adapted, with its authors and licences. */
const PORTED: [string, preact.ComponentChildren][] = [
  [
    "Kiwi protocol",
    <>
      The KiwiSDR protocol and its behaviour, ported from the Web-888 server (RaspSDR/server, KiwiSDR-derived; LGPL-2.0-or-later and GPL), © 2014-2023 John Seamons ZL4VO/KF6VO.
    </>,
  ],
  [
    "FT8 / FT4",
    <>
      Code tables and the decoder's first structure from <A href="https://github.com/kgoba/ft8_lib">ft8_lib</A> (MIT, © 2018 Kārlis Goba); the protocol by Franke, Somerville and Taylor (QEX 2020).
    </>,
  ],
  ["WSPR", <>Ported from wsprd (GPL-3.0-or-later, © 2001-2018 Joe Taylor K1JT, 2014-2018 Steven Franke K9AN), with Phil Karn KA9Q's Fano decoder.</>],
  [
    "JTTY",
    <>
      Ported from <A href="https://github.com/j0uni/jtty-cuf">jtty-cuf</A> (MIT, © 2026 Jouni OH3CUF); the mode is the WSJT-X developers' work.
    </>,
  ],
  ["JS8", <>Written with JS8Call (GPL-3.0) as the reference.</>],
  ["HFDL", <>Ground stations as dumphfdl's system table lists them.</>],
  [
    "Opus",
    <>
      <A href="https://github.com/RealEffect/opus1">opus1</A> (BSD-3-Clause), a Rust implementation of Opus following libopus (© Xiph.Org, Skype, Jean-Marc Valin and others).
    </>,
  ],
  [
    "Propagation",
    <>
      <A href="https://github.com/jonathanmcsweet/hfcast-engine">hfcast</A> (Apache-2.0, © 2026 Jonathan McSweet), a translation of VOACAP (Voice of America; NTIA/ITS, a US Government work; maintained by Greg Hand; voacapl by J.A. Watson, CC0). NTIA/ITS makes no warranty as to its
      accuracy and does not endorse its use.
    </>,
  ],
];

/** Data built in or fetched. */
const DATA: [string, preact.ComponentChildren][] = [
  [
    "Ionospheric maps",
    <>
      VOACAP's coefficients: NTIA/ITS work, and CCIR Reports 322 (atmospheric noise) and 340 (foF2, M(3000)F2), which ITU-R Study Group 3 publishes for free use in its{" "}
      <A href="https://github.com/ITU-R-Study-Group-3/ITU-R-HF">P.372 and P.533 software</A>. Not covered by the GPL.
    </>,
  ],
  [
    "Shortwave schedules",
    <>
      <A href="http://www.eibispace.de/">EiBi</A> by Eike Bierwirth, free to use and distribute.
    </>,
  ],
  [
    "Map",
    <>
      <A href="https://www.naturalearthdata.com/">Natural Earth</A> (public domain), via world-atlas.
    </>,
  ],
  [
    "Space weather",
    <>
      Sunspot predictions and the K index from <A href="https://www.swpc.noaa.gov/">NOAA SWPC</A> (when the owner lets the receiver fetch them).
    </>,
  ],
  [
    "DX spots",
    <>
      From <A href="https://spothole.app/">Spothole</A> (DX clusters, POTA, SOTA and others), when enabled.
    </>,
  ],
  ["Band plans, stations", <>Our own compilation from the IARU's, the ITU's and the operators' publications.</>],
];

function Table({ rows }: { rows: [string, preact.ComponentChildren][] }) {
  return (
    <dl class="info-facts lic-list">
      {rows.map(([k, v]) => (
        <Fragment key={k}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </Fragment>
      ))}
    </dl>
  );
}

function LibList({ title, libs }: { title: string; libs: Lib[] }) {
  return (
    <details class="lic-libs">
      <summary>
        {title} ({libs.length})
      </summary>
      <ul>
        {libs.map((l) => (
          <li key={`${l.name}@${l.version}`}>
            <A href={l.url}>{l.name}</A> {l.version} <span class="dim">{l.license}</span>
          </li>
        ))}
      </ul>
    </details>
  );
}

export function Licences({ server }: { server: string }) {
  const [libs, setLibs] = useState<Libs | null>(null);
  const [err, setErr] = useState("");
  useEffect(() => {
    fetch(api("api/info/licences"))
      .then((r) => (r.ok ? (r.json() as Promise<Libs>) : Promise.reject(new Error(`${r.status}`))))
      .then(setLibs)
      .catch((e: Error) => setErr(`The library list didn't load (${e.message}).`));
  }, []);
  return (
    <>
      <h3>Licences and credits</h3>
      <p>
        {server}: free software under the <A href="https://www.gnu.org/licenses/gpl-3.0.html">GNU GPL, version 3 or later</A>. Sources: <A href="https://github.com/jeskko/web888-ng">the server and web UI</A>,{" "}
        <A href="https://github.com/jeskko/web888-ng-hw">the FPGA design and driver</A>, <A href="https://github.com/jeskko/web888-ng-os">the OS image</A>. Not affiliated with the Web-888 / RX-888 vendor.
      </p>
      <h4>AI</h4>
      <p>
        The CW decoder and the signal identifier use small neural networks, trained by this project on synthetic signals; they run in your browser, and what they say can be wrong. Much of web888-ng's code and
        documentation was written with an AI coding assistant (Anthropic's Claude) at the project author's direction.
      </p>
      <h4>Code it builds on</h4>
      <Table rows={PORTED} />
      <h4>Data</h4>
      <Table rows={DATA} />
      <h4>Libraries</h4>
      {libs ? (
        <>
          <LibList title="In the server (Rust crates)" libs={libs.rust} />
          <LibList title="In this page" libs={libs.npm} />
          <p class="dim">Each under its own licence, as listed; their texts are in their sources.</p>
        </>
      ) : (
        <p class="dim">{err || "Loading…"}</p>
      )}
    </>
  );
}
