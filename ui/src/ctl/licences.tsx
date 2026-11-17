// SPDX-License-Identifier: GPL-3.0-or-later
// The info box's Licences view: web888d's own licence and sources, the
// code it was ported from, the data it carries or fetches, and the
// third-party libraries in this build (`/api/info/licences`, written by
// `scripts/third-party.py`).

import { Fragment } from "preact";
import { useEffect, useState } from "preact/hooks";
import { api } from "../state.ts";
import { t, tx } from "../i18n.ts";

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
const ported = (): [string, preact.ComponentChildren][] => [
  [
    "Kiwi protocol",
    t(
      "The KiwiSDR protocol and its behaviour, ported from the Web-888 server (RaspSDR/server, KiwiSDR-derived; LGPL-2.0-or-later and GPL), © 2014-2023 John Seamons ZL4VO/KF6VO.",
    ),
  ],
  [
    "FT8 / FT4",
    tx("Code tables and the decoder's first structure from {lib} (MIT, © 2018 Kārlis Goba); the protocol by Franke, Somerville and Taylor (QEX 2020).", {
      lib: <A href="https://github.com/kgoba/ft8_lib">ft8_lib</A>,
    }),
  ],
  ["WSPR", t("Ported from wsprd (GPL-3.0-or-later, © 2001-2018 Joe Taylor K1JT, 2014-2018 Steven Franke K9AN), with Phil Karn KA9Q's Fano decoder.")],
  [
    "JTTY",
    tx("Ported from {lib} (MIT, © 2026 Jouni OH3CUF); the mode is the WSJT-X developers' work.", { lib: <A href="https://github.com/j0uni/jtty-cuf">jtty-cuf</A> }),
  ],
  ["JS8", t("Written with JS8Call (GPL-3.0) as the reference.")],
  ["HFDL", t("Ground stations as dumphfdl's system table lists them.")],
  [
    "Opus",
    tx("{lib} (BSD-3-Clause), a Rust implementation of Opus following libopus (© Xiph.Org, Skype, Jean-Marc Valin and others).", {
      lib: <A href="https://github.com/RealEffect/opus1">opus1</A>,
    }),
  ],
  [
    t("Propagation"),
    tx(
      "{lib} (Apache-2.0, © 2026 Jonathan McSweet), a translation of VOACAP (Voice of America; NTIA/ITS, a US Government work; maintained by Greg Hand; voacapl by J.A. Watson, CC0). NTIA/ITS makes no warranty as to its accuracy and does not endorse its use.",
      { lib: <A href="https://github.com/jonathanmcsweet/hfcast-engine">hfcast</A> },
    ),
  ],
];

/** Data built in or fetched. */
const data = (): [string, preact.ComponentChildren][] => [
  [
    t("Ionospheric maps"),
    tx(
      "VOACAP's coefficients: NTIA/ITS work, and CCIR Reports 322 (atmospheric noise) and 340 (foF2, M(3000)F2), which ITU-R Study Group 3 publishes for free use in its {sw}. Not covered by the GPL.",
      { sw: <A href="https://github.com/ITU-R-Study-Group-3/ITU-R-HF">{t("P.372 and P.533 software")}</A> },
    ),
  ],
  [
    t("Shortwave schedules"),
    tx("{lib} by Eike Bierwirth, free to use and distribute.", { lib: <A href="http://www.eibispace.de/">EiBi</A> }),
  ],
  [
    t("Map"),
    tx("{lib} (public domain), via world-atlas.", { lib: <A href="https://www.naturalearthdata.com/">Natural Earth</A> }),
  ],
  [
    t("Space weather"),
    tx("Sunspot predictions and the K index from {lib} (when the owner lets the receiver fetch them).", { lib: <A href="https://www.swpc.noaa.gov/">NOAA SWPC</A> }),
  ],
  [
    t("DX spots"),
    tx("From {lib} (DX clusters, POTA, SOTA and others), when enabled.", { lib: <A href="https://spothole.app/">Spothole</A> }),
  ],
  [t("Band plans, stations"), t("Our own compilation from the IARU's, the ITU's and the operators' publications.")],
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
      .catch((e: Error) => setErr(t("The library list didn't load ({msg}).", { msg: e.message })));
  }, []);
  return (
    <>
      <h3>{t("Licences and credits")}</h3>
      <p>
        {tx("{server}: free software under the {gpl}. Sources: {web}, {hw}, {os}. Not affiliated with the Web-888 / RX-888 vendor.", {
          server,
          gpl: <A href="https://www.gnu.org/licenses/gpl-3.0.html">{t("GNU GPL, version 3 or later")}</A>,
          web: <A href="https://github.com/jeskko/web888-ng">{t("the server and web UI")}</A>,
          hw: <A href="https://github.com/jeskko/web888-ng-hw">{t("the FPGA design and driver")}</A>,
          os: <A href="https://github.com/jeskko/web888-ng-os">{t("the OS image")}</A>,
        })}
      </p>
      {/* i18n: AI = artificial intelligence */}
      <h4>{t("AI")}</h4>
      <p>
        {t(
          "The CW decoder and the signal identifier use small neural networks, trained by this project on synthetic signals; they run in your browser, and what they say can be wrong. Much of web888-ng's code and documentation was written with an AI coding assistant (Anthropic's Claude) at the project author's direction.",
        )}
      </p>
      <h4>{t("Code it builds on")}</h4>
      <Table rows={ported()} />
      <h4>{t("Data")}</h4>
      <Table rows={data()} />
      <h4>{t("Libraries")}</h4>
      {libs ? (
        <>
          <LibList title={t("In the server (Rust crates)")} libs={libs.rust} />
          <LibList title={t("In this page")} libs={libs.npm} />
          <p class="dim">{t("Each under its own licence, as listed; their texts are in their sources.")}</p>
        </>
      ) : (
        <p class="dim">{err || t("Loading…")}</p>
      )}
    </>
  );
}
