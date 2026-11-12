// SPDX-License-Identifier: GPL-3.0-or-later
// The map: stations heard (the skimmers' spots, this browser's decodes,
// live DX spots) with great circle lines from where they were heard, on
// a vector world map served by the receiver itself (Natural Earth, from
// the world-atlas package). Mercator, or azimuthal equidistant centred
// anywhere (drag to turn the globe under it); the greyline on top.

import { useSignal } from "@preact/signals";
import { geoAzimuthalEquidistant, geoCircle, geoGraticule, geoMercator, geoPath, type GeoPermissibleObjects, type GeoProjection } from "d3-geo";
import { useEffect, useMemo, useRef } from "preact/hooks";
import { feature, mesh } from "topojson-client";
import type { Topology } from "topojson-specification";
import { formatKhz } from "../fmt.ts";
import { bearing, distanceKm, gridToLatLon, latLonToGrid, subsolar, GridBook, type LatLon } from "../geo.ts";
import { bandColour, bandsOf, browserPoints, dxPoints, LAYERS, skimmerPoints, type Layer, type MapPoint } from "../map.ts";
import { utcTime } from "../spots.ts";
import { api, digiRows, mapDx, mapInfo, mapOpen, mapOpts, pickMapPoint, setMapOpts, showMap, spots, type MapOpts } from "../state.ts";

const AGES = [5, 15, 30, 60, 180];
/** Lines drawn at most (the newest), and while dragging. */
const MAX_LINES = 1500;
const MAX_LINES_MOVING = 200;

export function MapCtl() {
  const open = mapOpen.value;
  return (
    <div class="group">
      <button class={open ? "on" : ""} onClick={() => showMap(!open)} title="Map of the stations heard">
        Map
      </button>
    </div>
  );
}

interface World {
  land: GeoPermissibleObjects;
  borders: GeoPermissibleObjects;
}

/** The world at two resolutions, loaded once on first use. */
let worlds: Promise<[World, World]> | null = null;
function loadWorlds(): Promise<[World, World]> {
  const one = async (res: string): Promise<World> => {
    const r = await fetch(api(`map/countries-${res}.json`));
    if (!r.ok) throw new Error(`map/countries-${res}.json: HTTP ${r.status}`);
    const topo = (await r.json()) as Topology;
    const countries = topo.objects.countries as Parameters<typeof feature>[1];
    return {
      land: feature(topo, countries) as GeoPermissibleObjects,
      borders: mesh(topo, countries as never, (a, b) => a !== b) as GeoPermissibleObjects,
    };
  };
  worlds ??= Promise.all([one("110m"), one("50m")]).catch((e: unknown) => {
    worlds = null;
    throw e;
  });
  return worlds;
}

/** Where the map looks: its centre and zoom (1: the whole world). */
interface View {
  lon: number;
  lat: number;
  k: number;
}

function projection(kind: MapOpts["proj"], v: View, w: number, h: number): GeoProjection {
  if (kind === "merc") {
    const base = w / (2 * Math.PI);
    return geoMercator()
      .rotate([-v.lon, 0])
      .center([0, v.lat])
      .scale(base * v.k)
      .translate([w / 2, h / 2])
      .precision(0.5);
  }
  const base = Math.min(w, h) / 2 / Math.PI;
  return geoAzimuthalEquidistant()
    .rotate([-v.lon, -v.lat])
    .scale(base * v.k * 0.98)
    .translate([w / 2, h / 2])
    .clipAngle(179.9)
    .precision(0.5);
}

const css = (name: string, fallback: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;

function Tooltip({ p, rx }: { p: MapPoint; rx: LatLon | null }) {
  const from = p.from ?? rx;
  const km = from ? Math.round(distanceKm(from, p.pos)) : null;
  const deg = from ? Math.round(bearing(from, p.pos)) : null;
  const where = p.grid ?? latLonToGrid(p.pos, 4);
  return (
    <>
      <b>{p.call}</b> {where}
      <br />
      {p.band} {p.mode} {formatKhz(p.hz)} kHz{p.snr !== undefined ? ` · ${p.snr} dB` : ""} · {utcTime(p.t)}
      {km !== null && (
        <>
          <br />
          {km} km, {deg}° {p.layer === "dx" ? "from the spotter" : "from here"}
        </>
      )}
      <br />
      <span class="dim">{p.text}</span>
    </>
  );
}

export function MapPanel() {
  const canvas = useRef<HTMLCanvasElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const view = useSignal<View | null>(null);
  const hover = useSignal<{ p: MapPoint; x: number; y: number } | null>(null);
  const world = useSignal<[World, World] | null>(null);
  const err = useSignal("");
  const moving = useSignal(false);
  const book = useRef(new GridBook());
  const now = useSignal(Date.now() / 1000);
  const size = useSignal(0);
  const shown = useRef<{ p: MapPoint; x: number; y: number }[]>([]);
  const pointers = useRef(new Map<number, [number, number]>());
  const pinch = useRef(0);
  /** Where a press began and the station under it (a click, if it
   * doesn't move). */
  const press = useRef<{ x: number; y: number; hit: MapPoint | null } | null>(null);
  const open = mapOpen.value;

  // the world, the clock (greyline, ages)
  useEffect(() => {
    if (!open) return;
    loadWorlds()
      .then((w) => (world.value = w))
      .catch((e: unknown) => (err.value = `The world map didn't load: ${String(e)}`));
    const id = window.setInterval(() => (now.value = Date.now() / 1000), 30_000);
    return () => window.clearInterval(id);
  }, [open]);

  const info = mapInfo.value;
  const rx = info?.locator ? gridToLatLon(info.locator) : null;
  // the first view: the receiver in the middle (not while rendering:
  // a signal set then renders again outside Preact's render)
  useEffect(() => {
    if (open && !view.value && info) view.value = { lon: rx?.[1] ?? 0, lat: rx?.[0] ?? 0, k: 1 };
  }, [open, info]);

  const o = mapOpts.value;
  const sp = spots.value;
  const rows = digiRows.value;
  const dxs = mapDx.value;
  const t = now.value;
  const rxKey = rx?.join(",");
  const all = useMemo(() => {
    const out: MapPoint[] = [];
    if (!open) return out;
    const oldest = t - o.age * 60;
    // the book learns from everything heard, whatever is shown
    const lists: [Layer, MapPoint[]][] = [
      ["skimmer", skimmerPoints(sp, book.current, rx, oldest)],
      ["browser", browserPoints(rows, book.current, rx, oldest)],
      ["dx", dxPoints(dxs, oldest)],
    ];
    for (const [id, list] of lists) if (o.layers.includes(id)) out.push(...list);
    return out;
  }, [open, sp, rows, dxs, t, o, rxKey]);
  const bands = useMemo(() => bandsOf(all), [all]);
  const points = useMemo(() => (o.band ? all.filter((p) => p.band === o.band) : all.slice()).sort((a, b) => a.t - b.t), [all, o.band]);

  // draw
  useEffect(() => {
    const c = canvas.current;
    const v = view.value;
    if (!open || !c || !v) return;
    void size.value;
    const frame = requestAnimationFrame(() => {
      const dpr = window.devicePixelRatio || 1;
      const w = c.clientWidth;
      const h = c.clientHeight;
      if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
        c.width = Math.round(w * dpr);
        c.height = Math.round(h * dpr);
      }
      const g = c.getContext("2d")!;
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
      g.clearRect(0, 0, w, h);
      const proj = projection(o.proj, v, w, h);
      const path = geoPath(proj, g);
      const stroke = (obj: GeoPermissibleObjects, colour: string, width = 1) => {
        g.beginPath();
        path(obj);
        g.strokeStyle = colour;
        g.lineWidth = width;
        g.stroke();
      };
      const fill = (obj: GeoPermissibleObjects, colour: string) => {
        g.beginPath();
        path(obj);
        g.fillStyle = colour;
        g.fill();
      };
      fill({ type: "Sphere" }, css("--map-sea", "#0e1b2e"));
      // Maidenhead fields (20° x 10°)
      stroke(geoGraticule().step([20, 10]).extent([[-180, -90], [180, 90.001]])(), "rgba(140,160,190,0.18)");
      const wd = world.value;
      if (wd) {
        // the finer world when zoomed in and still
        const res = v.k >= 3 && !moving.value ? wd[1] : wd[0];
        fill(res.land, css("--map-land", "#3a4a63"));
        stroke(res.land, "rgba(200,215,235,0.55)", 0.7);
        stroke(res.borders, "rgba(200,215,235,0.3)", 0.5);
      }
      if (o.greyline) {
        const [la, lo] = subsolar(now.value);
        // the antipode of the sun: the middle of the night
        const night: [number, number] = [lo > 0 ? lo - 180 : lo + 180, -la];
        fill(geoCircle().center(night).radius(96)(), "rgba(0,0,0,0.12)");
        fill(geoCircle().center(night).radius(90)(), "rgba(0,0,0,0.22)");
      }
      stroke({ type: "Sphere" }, "rgba(170,185,205,0.5)");
      // lines, then points (newest on top)
      if (o.lines) {
        const max = moving.value ? MAX_LINES_MOVING : MAX_LINES;
        g.globalAlpha = 0.45;
        for (const p of points.slice(-max)) {
          if (!p.from) continue;
          stroke(
            {
              type: "LineString",
              coordinates: [
                [p.from[1], p.from[0]],
                [p.pos[1], p.pos[0]],
              ],
            },
            bandColour(p.band),
            1,
          );
        }
        g.globalAlpha = 1;
      }
      const placed: { p: MapPoint; x: number; y: number }[] = [];
      const centre = proj.invert?.([w / 2, h / 2]);
      for (const p of points) {
        const xy = proj([p.pos[1], p.pos[0]]);
        if (!xy || !Number.isFinite(xy[0]) || xy[0] < -10 || xy[1] < -10 || xy[0] > w + 10 || xy[1] > h + 10) continue;
        // azimuthal: the far side's edge is clipped
        if (o.proj === "azim" && centre && distanceKm([centre[1], centre[0]], p.pos) > 0.999 * Math.PI * 6371) continue;
        const [x, y] = xy;
        g.fillStyle = bandColour(p.band);
        g.strokeStyle = "rgba(0,0,0,0.6)";
        g.lineWidth = 1;
        g.beginPath();
        if (p.layer === "dx") g.rect(x - 3, y - 3, 6, 6);
        else if (p.layer === "browser") {
          g.moveTo(x, y - 4);
          g.lineTo(x + 4, y);
          g.lineTo(x, y + 4);
          g.lineTo(x - 4, y);
          g.closePath();
        } else g.arc(x, y, 3, 0, 2 * Math.PI);
        g.fill();
        g.stroke();
        placed.push({ p, x, y });
      }
      // calls beside the points, where they fit (the newest first)
      if (v.k >= 2 || placed.length < 150) {
        g.font = "11px system-ui, sans-serif";
        g.fillStyle = "rgba(230,236,245,0.9)";
        const taken: [number, number, number, number][] = [];
        for (const { p, x, y } of placed.slice().reverse()) {
          const tw = g.measureText(p.call).width;
          const r: [number, number, number, number] = [x + 5, y - 6, x + 5 + tw, y + 6];
          if (taken.some((t) => r[0] < t[2] && r[2] > t[0] && r[1] < t[3] && r[3] > t[1])) continue;
          taken.push(r);
          g.fillText(p.call, x + 5, y + 4);
        }
      }
      // the receiver
      if (rx) {
        const xy = proj([rx[1], rx[0]]);
        if (xy) {
          g.beginPath();
          g.arc(xy[0], xy[1], 6, 0, 2 * Math.PI);
          g.strokeStyle = "#ffffff";
          g.lineWidth = 2;
          g.stroke();
          g.beginPath();
          g.arc(xy[0], xy[1], 2, 0, 2 * Math.PI);
          g.fillStyle = "#ffffff";
          g.fill();
        }
      }
      shown.current = placed;
    });
    return () => cancelAnimationFrame(frame);
  }, [open, points, view.value, world.value, moving.value, size.value, o, t, rxKey]);

  // redraw on resize
  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const ro = new ResizeObserver(() => size.value++);
    ro.observe(c);
    return () => ro.disconnect();
  }, [open]);

  if (!open) return null;

  // drag turns or pans, the wheel and pinches zoom
  const onDown = (e: PointerEvent) => {
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    pointers.current.set(e.pointerId, [e.clientX, e.clientY]);
    press.current = { x: e.clientX, y: e.clientY, hit: hover.value?.p ?? null };
    moving.value = true;
    hover.value = null;
  };
  const onMove = (e: PointerEvent) => {
    const v = view.value;
    const c = canvas.current;
    if (!v || !c) return;
    const ps = pointers.current;
    const last = ps.get(e.pointerId);
    if (!last) {
      // hover: the nearest point within 10 px
      const r = c.getBoundingClientRect();
      const x = e.clientX - r.left;
      const y = e.clientY - r.top;
      let best: { p: MapPoint; x: number; y: number } | null = null;
      let bd = 100;
      for (const s of shown.current) {
        const d = (s.x - x) ** 2 + (s.y - y) ** 2;
        if (d < bd) {
          bd = d;
          best = s;
        }
      }
      hover.value = best ? { p: best.p, x, y } : null;
      return;
    }
    const cur: [number, number] = [e.clientX, e.clientY];
    ps.set(e.pointerId, cur);
    if (ps.size === 2) {
      const [a, b] = [...ps.values()];
      const d = Math.hypot(a[0] - b[0], a[1] - b[1]);
      if (pinch.current) view.value = { ...v, k: Math.min(64, Math.max(0.5, (v.k * d) / pinch.current)) };
      pinch.current = d;
      return;
    }
    const w = c.clientWidth;
    const h = c.clientHeight;
    const scale = o.proj === "merc" ? (w / (2 * Math.PI)) * v.k : (Math.min(w, h) / 2 / Math.PI) * v.k;
    const deg = 180 / Math.PI / scale;
    const lon = ((v.lon - (cur[0] - last[0]) * deg + 540) % 360) - 180;
    const lat = Math.max(-80, Math.min(80, v.lat + (cur[1] - last[1]) * deg));
    view.value = { ...v, lon, lat };
  };
  const onUp = (e: PointerEvent) => {
    const ps = pointers.current;
    ps.delete(e.pointerId);
    if (ps.size < 2) pinch.current = 0;
    if (ps.size === 0) moving.value = false;
  };
  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    const v = view.value;
    if (v) view.value = { ...v, k: Math.min(64, Math.max(0.5, v.k * Math.exp(-e.deltaY * 0.0015))) };
  };
  const onClick = (e: MouseEvent) => {
    const pr = press.current;
    press.current = null;
    if (pr?.hit && Math.hypot(e.clientX - pr.x, e.clientY - pr.y) < 5) pickMapPoint(pr.hit);
  };
  const home = () => (view.value = { lon: rx?.[1] ?? 0, lat: o.proj === "merc" ? 30 : (rx?.[0] ?? 0), k: 1 });
  const toggle = (id: Layer) => setMapOpts({ layers: o.layers.includes(id) ? o.layers.filter((l) => l !== id) : [...o.layers, id] });
  const hv = hover.value;

  return (
    <aside class="map">
      <div class="spots-bar">
        <select value={o.proj} onChange={(e) => setMapOpts({ proj: (e.target as HTMLSelectElement).value as MapOpts["proj"] })} title="Projection">
          <option value="azim">Great circle</option>
          <option value="merc">Mercator</option>
        </select>
        {LAYERS.map((l) => (
          <label key={l.id} title={l.title}>
            <input type="checkbox" checked={o.layers.includes(l.id)} onChange={() => toggle(l.id)} /> {l.name}
          </label>
        ))}
        <select value={o.band} onChange={(e) => setMapOpts({ band: (e.target as HTMLSelectElement).value })} title="Band">
          <option value="">All bands</option>
          {(o.band && !bands.includes(o.band) ? [o.band, ...bands] : bands).map((b) => (
            <option value={b}>{b}</option>
          ))}
        </select>
        <select value={o.age} onChange={(e) => setMapOpts({ age: Number((e.target as HTMLSelectElement).value) })} title="Heard in the last">
          {AGES.map((a) => (
            <option value={a}>{a < 60 ? `${a} min` : `${a / 60} h`}</option>
          ))}
        </select>
        <label title="Great circle lines from where each station was heard">
          <input type="checkbox" checked={o.lines} onChange={(e) => setMapOpts({ lines: e.currentTarget.checked })} /> Lines
        </label>
        <label title="Night and twilight now">
          <input type="checkbox" checked={o.greyline} onChange={(e) => setMapOpts({ greyline: e.currentTarget.checked })} /> Greyline
        </label>
        <span class="dim num" title="Stations shown">
          {points.length}
        </span>
        <button onClick={home} title="Centre on this receiver">
          ⌖
        </button>
        <button class="close" onClick={() => showMap(false)} title="Close">
          ✕
        </button>
      </div>
      <div class="map-box" ref={box}>
        <canvas
          ref={canvas}
          class={hv && (hv.p.spot || hv.p.tune) ? "pick" : ""}
          onPointerDown={onDown}
          onPointerMove={onMove}
          onPointerUp={onUp}
          onPointerCancel={onUp}
          onPointerLeave={() => (hover.value = null)}
          onWheel={onWheel}
          onClick={onClick}
        />
        {hv && (
          <div class="map-tip" style={{ left: `${hv.x + 12}px`, top: `${hv.y + 12}px` }}>
            <Tooltip p={hv.p} rx={rx} />
          </div>
        )}
        {err.value && <p class="warn map-note">{err.value}</p>}
        {info && !rx && <p class="dim map-note">This receiver's locator isn't set (admin: station): no lines from it.</p>}
      </div>
    </aside>
  );
}
