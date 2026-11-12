// SPDX-License-Identifier: GPL-3.0-or-later
// The map: stations heard (the skimmers' spots, this browser's decodes,
// live DX spots) with great circle lines from where they were heard, on
// a vector world map served by the receiver itself (Natural Earth, from
// the world-atlas package). Mercator, or azimuthal equidistant centred
// anywhere (drag to turn the globe under it); the greyline on top.

import { useSignal } from "@preact/signals";
import {
  geoAzimuthalEquidistant,
  geoCentroid,
  geoCircle,
  geoDistance,
  geoGraticule,
  geoMercator,
  geoOrthographic,
  geoPath,
  type GeoPermissibleObjects,
  type GeoProjection,
} from "d3-geo";
import { useEffect, useMemo, useRef } from "preact/hooks";
import { feature } from "topojson-client";
import type { Topology } from "topojson-specification";
import { formatKhz } from "../fmt.ts";
import { bearing, distanceKm, gridToLatLon, latLonToGrid, subsolar, GridBook, type LatLon } from "../geo.ts";
import { bandColour, bandsOf, browserPoints, dxPoints, heardTimes, LAYERS, skimmerPoints, type Layer, type MapPoint } from "../map.ts";
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

/** A country, with the circle around it (centre, radius in radians) to
 * leave it out quickly when it is out of view. */
interface Country {
  f: GeoPermissibleObjects;
  c: [number, number];
  r: number;
}
type World = Country[];

/** Every [lon, lat] of a geometry. */
function eachPoint(c: unknown, out: (p: [number, number]) => void): void {
  if (Array.isArray(c) && typeof c[0] === "number") out(c as [number, number]);
  else if (Array.isArray(c)) for (const x of c) eachPoint(x, out);
}

/** The world at two resolutions, loaded once on first use. */
let worlds: Promise<[World, World]> | null = null;
function loadWorlds(): Promise<[World, World]> {
  const one = async (res: string): Promise<World> => {
    const r = await fetch(api(`map/countries-${res}.json`));
    if (!r.ok) throw new Error(`map/countries-${res}.json: HTTP ${r.status}`);
    const topo = (await r.json()) as Topology;
    const fc = feature(topo, topo.objects.countries as Parameters<typeof feature>[1]) as unknown as {
      features: { geometry: { coordinates: unknown } | null }[];
    };
    return fc.features
      .filter((f) => f.geometry)
      .map((f) => {
        const g = f as unknown as GeoPermissibleObjects;
        const c = geoCentroid(g as never);
        let r = 0;
        eachPoint(f.geometry!.coordinates, (p) => (r = Math.max(r, geoDistance(c, p))));
        return { f: g, c, r };
      });
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

/** The projection of view `v` on a `w` x `h` canvas; `coarse` (while
 * moving): curves resampled to a few pixels, not under one. Clipped to
 * the canvas: zoomed in, most of the world isn't drawn at all. */
function projection(kind: MapOpts["proj"], v: View, w: number, h: number, coarse: boolean): GeoProjection {
  const p =
    kind === "merc"
      ? geoMercator().rotate([-v.lon, 0]).center([0, v.lat])
      : kind === "globe"
        ? geoOrthographic().rotate([-v.lon, -v.lat]).clipAngle(90)
        : geoAzimuthalEquidistant().rotate([-v.lon, -v.lat]).clipAngle(179.9);
  p.scale(scaleOf(kind, v, w, h));
  return p
    .translate([w / 2, h / 2])
    .clipExtent([
      [-2, -2],
      [w + 2, h + 2],
    ])
    .precision(coarse ? 3 : 0.7);
}

/** Pixels per radian at the centre of the view. */
function scaleOf(kind: MapOpts["proj"], v: View, w: number, h: number): number {
  if (kind === "merc") return (w / (2 * Math.PI)) * v.k;
  // the globe's radius, or the azimuthal map's (pi radians to its edge)
  const r = (Math.min(w, h) / 2) * 0.98 * v.k;
  return kind === "globe" ? r : r / Math.PI;
}

/** The view's centre on the globe and how far from it (radians) the
 * canvas reaches: what is farther isn't drawn. */
function reach(proj: GeoProjection, w: number, h: number): { c: [number, number]; r: number } {
  const c = proj.invert?.([w / 2, h / 2]);
  if (!c) return { c: [0, 0], r: Math.PI };
  let r = 0;
  for (const xy of [
    [0, 0],
    [w, 0],
    [0, h],
    [w, h],
    [w / 2, 0],
    [w / 2, h],
    [0, h / 2],
    [w, h / 2],
  ] as [number, number][]) {
    const g = proj.invert?.(xy);
    if (!g || !Number.isFinite(g[0])) return { c, r: Math.PI };
    r = Math.max(r, geoDistance(c, g));
  }
  // an edge can bulge past its corners (azimuthal): some margin
  return { c, r: Math.min(Math.PI, r * 1.15 + 0.02) };
}

/** The base map: sea, Maidenhead fields, the countries in view. */
function drawBase(g: CanvasRenderingContext2D, proj: GeoProjection, world: World | null, w: number, h: number): void {
  const path = geoPath(proj, g);
  g.beginPath();
  path({ type: "Sphere" });
  g.fillStyle = css("--map-sea", "#0e1b2e");
  g.fill();
  g.beginPath();
  path(geoGraticule().step([20, 10]).extent([[-180, -90], [180, 90.001]])());
  g.strokeStyle = "rgba(140,160,190,0.18)";
  g.lineWidth = 1;
  g.stroke();
  if (world) {
    const { c, r } = reach(proj, w, h);
    const land = new Path2D();
    const lp = geoPath(proj, land as unknown as CanvasRenderingContext2D);
    for (const k of world) if (r >= Math.PI || geoDistance(c, k.c) - k.r <= r) lp(k.f);
    g.fillStyle = css("--map-land", "#3a4a63");
    g.fill(land);
    g.strokeStyle = "rgba(200,215,235,0.45)";
    g.lineWidth = 0.6;
    g.stroke(land);
  }
  g.beginPath();
  path({ type: "Sphere" });
  g.strokeStyle = "rgba(170,185,205,0.5)";
  g.lineWidth = 1;
  g.stroke();
}

/** The base map drawn last, for reuse while only the stations change,
 * and stretched or shifted while zooming or panning. */
interface Base {
  key: string;
  canvas: HTMLCanvasElement;
  view: View;
  proj: MapOpts["proj"];
  w: number;
  h: number;
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

/** Where a station's details go: beside the point, on the side with room
 * (a phone's map is narrow). */
function tipPlace(x: number, y: number, c: HTMLCanvasElement | null): Record<string, string> {
  const w = c?.clientWidth ?? 0;
  const h = c?.clientHeight ?? 0;
  const st: Record<string, string> = {};
  if (x > w / 2) st.right = `${Math.max(4, w - x + 12)}px`;
  else st.left = `${x + 12}px`;
  if (y > h / 2) st.bottom = `${Math.max(4, h - y + 12)}px`;
  else st.top = `${y + 12}px`;
  return st;
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
  const press = useRef<{ x: number; y: number; hit: MapPoint | null; touch: boolean } | null>(null);
  /** On a touch screen (no hover): the station a tap picked, shown until
   * the next tap; a second tap on it tunes there. */
  const picked = useSignal<MapPoint | null>(null);
  /** Labels' widths (measuring text is slow), and the wheel's end. */
  const widths = useRef(new Map<string, number>());
  const wheelEnd = useRef(0);
  const base = useRef<Base | null>(null);
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
    // uncertain decodes count once their call is heard again
    const heard = heardTimes(sp, rows);
    const lists: [Layer, MapPoint[]][] = [
      ["skimmer", skimmerPoints(sp, book.current, rx, oldest, heard)],
      ["browser", browserPoints(rows, book.current, rx, oldest, heard)],
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
      const t0 = performance.now();
      const coarse = moving.value;
      const dpr = window.devicePixelRatio || 1;
      const w = c.clientWidth;
      const h = c.clientHeight;
      if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
        c.width = Math.round(w * dpr);
        c.height = Math.round(h * dpr);
      }
      const g = c.getContext("2d")!;
      g.setTransform(1, 0, 0, 1, 0, 0);
      g.clearRect(0, 0, c.width, c.height);
      const proj = projection(o.proj, v, w, h, coarse);
      // the base map: as drawn last if the view is the same; stretched
      // (zoom) or shifted (Mercator pan) while moving; else drawn now
      const wd = world.value;
      const fine = v.k >= 3 && !coarse;
      const key = `${o.proj} ${v.lon} ${v.lat} ${v.k} ${w} ${h} ${dpr} ${fine} ${!!wd}`;
      const b = base.current;
      const same = b && b.proj === o.proj && b.w === w && b.h === h;
      if (b && b.key === key) {
        g.drawImage(b.canvas, 0, 0);
      } else if (coarse && same && b.view.lon === v.lon && b.view.lat === v.lat) {
        const s = v.k / b.view.k;
        g.drawImage(b.canvas, (c.width * (1 - s)) / 2, (c.height * (1 - s)) / 2, c.width * s, c.height * s);
      } else if (coarse && same && o.proj === "merc" && b.view.k === v.k) {
        const was = projection("merc", b.view, w, h, true).invert?.([w / 2, h / 2]);
        const xy = was && proj(was);
        if (xy) g.drawImage(b.canvas, (xy[0] - w / 2) * dpr, (xy[1] - h / 2) * dpr);
      } else {
        const bc = b?.canvas ?? document.createElement("canvas");
        bc.width = c.width;
        bc.height = c.height;
        const bg = bc.getContext("2d")!;
        bg.setTransform(dpr, 0, 0, dpr, 0, 0);
        drawBase(bg, proj, wd ? wd[fine ? 1 : 0] : null, w, h);
        base.current = { key, canvas: bc, view: v, proj: o.proj, w, h };
        g.drawImage(bc, 0, 0);
      }
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
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
      if (o.greyline) {
        const [la, lo] = subsolar(now.value);
        // the antipode of the sun: the middle of the night
        const night: [number, number] = [lo > 0 ? lo - 180 : lo + 180, -la];
        fill(geoCircle().center(night).radius(96)(), "rgba(0,0,0,0.09)");
        fill(geoCircle().center(night).radius(90)(), "rgba(0,0,0,0.16)");
      }
      // lines, one path per band, then points (newest on top)
      if (o.lines) {
        const byBand = new Map<string, [number, number][][]>();
        for (const p of points.slice(-(coarse ? MAX_LINES_MOVING : MAX_LINES))) {
          if (!p.from) continue;
          let l = byBand.get(p.band);
          if (!l) byBand.set(p.band, (l = []));
          l.push([
            [p.from[1], p.from[0]],
            [p.pos[1], p.pos[0]],
          ]);
        }
        g.globalAlpha = 0.45;
        for (const [band, coordinates] of byBand) stroke({ type: "MultiLineString", coordinates }, bandColour(band), 1);
        g.globalAlpha = 1;
      }
      const placed: { p: MapPoint; x: number; y: number }[] = [];
      const centre = proj.invert?.([w / 2, h / 2]);
      // the far side: past the azimuthal map's edge, behind the globe
      const far = o.proj === "globe" ? (Math.PI / 2) * 6371 : o.proj === "azim" ? 0.999 * Math.PI * 6371 : Infinity;
      for (const p of points) {
        const xy = proj([p.pos[1], p.pos[0]]);
        if (!xy || !Number.isFinite(xy[0]) || xy[0] < -10 || xy[1] < -10 || xy[0] > w + 10 || xy[1] > h + 10) continue;
        if (far < Infinity && centre && distanceKm([centre[1], centre[0]], p.pos) > far) continue;
        placed.push({ p, x: xy[0], y: xy[1] });
      }
      // the points, one path per band (later bands on top)
      const marks = new Map<string, typeof placed>();
      for (const s of placed) {
        let l = marks.get(s.p.band);
        if (!l) marks.set(s.p.band, (l = []));
        l.push(s);
      }
      g.strokeStyle = "rgba(0,0,0,0.6)";
      g.lineWidth = 1;
      for (const [band, list] of marks) {
        g.beginPath();
        for (const { p, x, y } of list) {
          if (p.layer === "dx") g.rect(x - 3, y - 3, 6, 6);
          else if (p.layer === "browser") {
            g.moveTo(x, y - 4);
            g.lineTo(x + 4, y);
            g.lineTo(x, y + 4);
            g.lineTo(x - 4, y);
            g.closePath();
          } else {
            g.moveTo(x + 3, y);
            g.arc(x, y, 3, 0, 2 * Math.PI);
          }
        }
        g.fillStyle = bandColour(band);
        g.fill();
        g.stroke();
      }
      // calls beside the points, where they fit (the newest first)
      if (v.k >= 2 || placed.length < 150) {
        g.font = "11px system-ui, sans-serif";
        g.fillStyle = "rgba(230,236,245,0.9)";
        const taken: [number, number, number, number][] = [];
        for (const { p, x, y } of placed.slice().reverse()) {
          let tw = widths.current.get(p.call);
          if (tw === undefined) widths.current.set(p.call, (tw = g.measureText(p.call).width));
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
      // how long a frame takes (checks: tools/cdp/map.mjs)
      c.dataset.drawMs = (performance.now() - t0).toFixed(1);
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

  /** The station drawn nearest to (x, y) within `r` px, in the canvas. */
  const nearest = (x: number, y: number, r: number) => {
    let best: { p: MapPoint; x: number; y: number } | null = null;
    let bd = r * r;
    for (const s of shown.current) {
      const d = (s.x - x) ** 2 + (s.y - y) ** 2;
      if (d < bd) {
        bd = d;
        best = s;
      }
    }
    return best;
  };

  // drag turns or pans, the wheel and pinches zoom
  const onDown = (e: PointerEvent) => {
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    pointers.current.set(e.pointerId, [e.clientX, e.clientY]);
    const touch = e.pointerType !== "mouse";
    press.current = { x: e.clientX, y: e.clientY, hit: touch ? null : (hover.value?.p ?? null), touch };
    moving.value = true;
    if (!touch) hover.value = null;
  };
  const onMove = (e: PointerEvent) => {
    const v = view.value;
    const c = canvas.current;
    if (!v || !c) return;
    const ps = pointers.current;
    const last = ps.get(e.pointerId);
    if (!last) {
      // hover (a mouse): the nearest point within 10 px
      if (e.pointerType !== "mouse") return;
      const r = c.getBoundingClientRect();
      const x = e.clientX - r.left;
      const y = e.clientY - r.top;
      const best = nearest(x, y, 10);
      hover.value = best ? { p: best.p, x, y } : null;
      return;
    }
    // a drag hides a tapped station's details
    if (picked.value && Math.hypot(e.clientX - (press.current?.x ?? 0), e.clientY - (press.current?.y ?? 0)) > 8) {
      picked.value = null;
      hover.value = null;
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
    const scale = scaleOf(o.proj, v, w, h);
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
    // coarse while the wheel turns, the full detail once it stops
    moving.value = true;
    window.clearTimeout(wheelEnd.current);
    wheelEnd.current = window.setTimeout(() => {
      if (pointers.current.size === 0) moving.value = false;
    }, 250);
    const v = view.value;
    if (v) view.value = { ...v, k: Math.min(64, Math.max(0.5, v.k * Math.exp(-e.deltaY * 0.0015))) };
  };
  const onClick = (e: MouseEvent) => {
    const pr = press.current;
    press.current = null;
    if (!pr) return;
    if (!pr.touch) {
      if (pr.hit && Math.hypot(e.clientX - pr.x, e.clientY - pr.y) < 5) pickMapPoint(pr.hit);
      return;
    }
    // a tap: show the station under the finger; the same one again tunes
    if (Math.hypot(e.clientX - pr.x, e.clientY - pr.y) >= 10) return;
    const c = canvas.current;
    if (!c) return;
    const r = c.getBoundingClientRect();
    const x = e.clientX - r.left;
    const y = e.clientY - r.top;
    const best = nearest(x, y, 24);
    if (best && best.p === picked.value && (best.p.spot || best.p.tune)) {
      pickMapPoint(best.p);
      return;
    }
    picked.value = best?.p ?? null;
    hover.value = best ? { p: best.p, x: best.x, y: best.y } : null;
  };
  const home = () => (view.value = { lon: rx?.[1] ?? 0, lat: o.proj === "merc" ? 30 : (rx?.[0] ?? 0), k: 1 });
  const toggle = (id: Layer) => setMapOpts({ layers: o.layers.includes(id) ? o.layers.filter((l) => l !== id) : [...o.layers, id] });
  const hv = hover.value;

  return (
    <aside class="map">
      <div class="spots-bar">
        <select value={o.proj} onChange={(e) => setMapOpts({ proj: (e.target as HTMLSelectElement).value as MapOpts["proj"] })} title="Projection">
          <option value="azim">Great circle</option>
          <option value="globe">Globe</option>
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
          onPointerLeave={(e) => e.pointerType === "mouse" && (hover.value = null)}
          onWheel={onWheel}
          onClick={onClick}
        />
        {hv && (
          <div class="map-tip" style={tipPlace(hv.x, hv.y, canvas.current)}>
            <Tooltip p={hv.p} rx={rx} />
            {picked.value === hv.p && (hv.p.spot || hv.p.tune) && <div class="dim">Tap it again to tune there.</div>}
          </div>
        )}
        {err.value && <p class="warn map-note">{err.value}</p>}
        {info && !rx && <p class="dim map-note">This receiver's locator isn't set (admin: station): no lines from it.</p>}
      </div>
    </aside>
  );
}
