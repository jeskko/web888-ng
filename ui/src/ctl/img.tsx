// SPDX-License-Identifier: GPL-3.0-or-later
// HF fax and SSTV decoded in this browser from this receiver's audio: the
// button, and beside the waterfall the pictures as they arrive (fax from
// its start tone or by hand, with slant and shift nudges; SSTV from its
// VIS code), a gallery of the last ones, PNG downloads.

import { useEffect, useRef, useState } from "preact/hooks";
import { t } from "../i18n.ts";
import { FAX_STATIONS, rgba, SSTV_DIALS, type Picture } from "../img.ts";
import { faxTrim, imgCmd, imgError, imgOn, imgState, imgVersion, pickImg, setImg, st, trimFax } from "../state.ts";

export function ImgCtl() {
  const on = imgOn.value;
  return (
    <div class="group">
      <button class={on ? "on" : ""} onClick={() => setImg(!on)} title={t("Decode HF fax (weather charts) and SSTV pictures from this receiver's audio (USB), in this browser")}>
        FAX/SSTV
      </button>
    </div>
  );
}

/** A picture on a canvas, drawn row by row as it grows. */
function PictureView({ p, cls }: { p: Picture; cls?: string }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const drawn = useRef({ rows: 0, pic: null as Picture | null });
  useEffect(() => {
    const c = ref.current;
    if (!c) return;
    const d = drawn.current;
    const h = p.height ?? Math.max(256, Math.ceil(p.rows / 256) * 256);
    if (d.pic !== p || c.width !== p.width || c.height !== h || p.rows < d.rows) {
      c.width = p.width;
      c.height = h;
      d.rows = 0;
      d.pic = p;
    }
    if (p.rows > d.rows) {
      const ctx = c.getContext("2d");
      ctx?.putImageData(new ImageData(rgba(p, d.rows, p.rows), p.width, p.rows - d.rows), 0, d.rows);
      d.rows = p.rows;
    }
  }, [p, p.version, p.rows]);
  return <canvas ref={ref} class={cls ?? "img-pic"} />;
}

function save(p: Picture): void {
  const c = document.createElement("canvas");
  c.width = p.width;
  c.height = Math.max(1, p.rows);
  c.getContext("2d")?.putImageData(new ImageData(rgba(p, 0, p.rows), p.width, p.rows), 0, 0);
  const stamp = new Date(p.t * 1000).toISOString().replace(/[-:]/g, "").slice(0, 15);
  c.toBlob((b) => {
    if (!b) return;
    const a = document.createElement("a");
    a.href = URL.createObjectURL(b);
    a.download = `${p.kind}-${stamp}-${Math.round(p.dial / 100) / 10}kHz.png`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  });
}

export function ImgPanel() {
  void imgVersion.value; // re-render on decoder output
  const [shown, setShown] = useState<Picture | null>(null);
  if (!imgOn.value) return null;
  const s = st.value;
  const { fax, sstv, gallery, status } = imgState;
  const current = shown ?? sstv ?? fax ?? gallery[0] ?? null;
  const fs = status?.fax;
  // i18n: IOC = index of cooperation (fax drum width/line pitch ratio); slant = image skew from the sampling-rate error, in ppm; start tone = the tone that begins a fax chart; VIS code = the digital header that names the SSTV mode
  const faxLine =
    fs?.state === "image"
      ? t("Fax: IOC {ioc}, {lines} lines, slant {slant} ppm", { ioc: fs.ioc, lines: fs.lines, slant: `${fs.slant > 0 ? "+" : ""}${fs.slant}` })
      : fs?.state === "start"
        ? t("Fax: start tone (IOC {ioc})", { ioc: fs.ioc })
        : t("Fax: waiting for a start tone");
  const sstvLine = status?.sstv ? t("SSTV: {mode}, {lines} lines", { mode: status.sstv.mode, lines: status.sstv.lines }) : t("SSTV: waiting for a VIS code");
  return (
    <aside class="spots img">
      <div class="spots-bar">
        <strong>FAX/SSTV</strong>
        <select
          value=""
          onChange={(e) => {
            const v = (e.target as HTMLSelectElement).value;
            pickImg(Number(v.slice(1)), v[0] === "f");
          }}
          title={t("Fax stations (the dial 1.9 kHz below) and SSTV calling frequencies, USB")}
        >
          <option value="" disabled>
            {t("Station…")}
          </option>
          {FAX_STATIONS.map((x) => (
            <option value={`f${x.hz}`}>{x.name}</option>
          ))}
          {SSTV_DIALS.map((x) => (
            <option value={`s${x.hz}`}>{x.name}</option>
          ))}
        </select>
        <button class="close" onClick={() => setImg(false)} title={t("Stop decoding")}>
          ✕
        </button>
      </div>
      <div class="spots-list">
        {imgError.value && <p class="warn">{imgError.value}</p>}
        {s && s.mode !== "usb" && <p class="dim">{t("Fax and SSTV are decoded in USB.")}</p>}
        <p class="dim img-status">
          {faxLine}
          <br />
          {sstvLine}
        </p>
        <div class="img-ctl">
          <button onClick={() => imgCmd(fs?.state === "image" ? "faxStop" : "faxStart")} title={t("Start a fax image now (no start tone heard), or end the one being received")}>
            {fs?.state === "image" ? t("Fax stop") : t("Fax start")}
          </button>
          <button onClick={() => trimFax(-50, 0)} // i18n: slant = image skew (lines lean) from a sample-rate error between sender and receiver
            title={t("Fax slant: lines lean left")}>
            ⟲
          </button>
          <button onClick={() => trimFax(50, 0)} title={t("Fax slant: lines lean right")}>
            ⟳
          </button>
          <button onClick={() => trimFax(0, -0.02)} title={t("Fax: move the image left")}>
            ◀
          </button>
          <button onClick={() => trimFax(0, 0.02)} title={t("Fax: move the image right")}>
            ▶
          </button>
          <span class="dim num" title={t("Manual fax slant (ppm) and shift")}>
            {faxTrim.value.ppm} ppm
          </span>
          {status?.sstv && (
            <button onClick={() => imgCmd("sstvStop")} title={t("End the SSTV image")}>
              {t("SSTV stop")}
            </button>
          )}
        </div>
        {current ? (
          <div>
            <div class="img-head">
              <span>
                {current.kind === "fax" ? "Fax" : "SSTV"} {current.mode} · {new Date(current.t * 1000).toISOString().slice(11, 16)} UTC{current.done ? "" : ` · ${t("receiving")}`}
              </span>
              <button onClick={() => save(current)} title={t("Download as PNG")}>
                {t("Save")}
              </button>
              {shown && (
                <button onClick={() => setShown(null)} title={t("Back to the picture being received")}>
                  {t("Live")}
                </button>
              )}
            </div>
            <PictureView p={current} />
          </div>
        ) : (
          <p class="dim">
            {/* i18n: fax station = a weather-chart radiofax transmitter; VIS code = the digital header that names the SSTV mode and starts a picture; Martin, Scottie, Robot, PD are SSTV mode families (names kept) */}
            {t("Nothing yet: tune a fax station (its start tone begins a chart; or start one by hand) or an SSTV frequency (a picture begins with its VIS code: Martin, Scottie, Robot, PD).")}
          </p>
        )}
        {gallery.length > 0 && (
          <div class="img-gallery">
            {gallery.map((p) => (
              <button class="img-thumb" onClick={() => setShown(p)} title={`${p.kind === "fax" ? "Fax" : "SSTV"} ${p.mode}, ${new Date(p.t * 1000).toISOString().slice(11, 16)} UTC`}>
                <PictureView p={p} cls="img-thumb-pic" />
              </button>
            ))}
          </div>
        )}
      </div>
    </aside>
  );
}
