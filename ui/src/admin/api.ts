// SPDX-License-Identifier: GPL-3.0-or-later
// The admin API (server/doc/m3-multi-user.md §7): JSON over fetch, with
// the login cookie. A 401 means the login ended (expired, or the admin
// password changed elsewhere): `onLoggedOut` takes the page back to the
// login form.

import { signal } from "@preact/signals";

export class ApiError extends Error {
  status: number;
  constructor(status: number, msg: string) {
    super(msg);
    this.status = status;
  }
}

/** Set when a call was refused for want of a login. */
export const loggedOut = signal(false);

function url(path: string): string {
  return location.pathname.replace(/[^/]*$/, "") + path;
}

export async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const r = await fetch(url(path), {
    method,
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = (await r.json().catch(() => ({}))) as T & { error?: string };
  if (!r.ok) {
    if (r.status === 401 && path.startsWith("api/admin/")) loggedOut.value = true;
    throw new ApiError(r.status, data.error ?? `${method} ${path}: ${r.status}`);
  }
  return data;
}

export const get = <T>(path: string) => call<T>("GET", path);

export interface Auth {
  role: "admin" | "listener" | null;
  admin_password: boolean;
  listener_password: boolean;
}

export interface Changes {
  live: boolean;
  restart: string[];
}

export interface Session {
  id: number;
  ip: string;
  class: string;
  since: number;
  hz: number | null;
  mode: string | null;
  share: boolean | null;
  channel: number | null;
  waiting: number | null;
  listen: number | null;
  audio_drops: number;
  wf_drops: number;
}

export interface LeaseRow {
  id: number;
  class: string;
  label: string;
  hz: number;
  channel: number | null;
  waiting: number | null;
}

export interface Stats {
  sessions: number;
  threads: { name: string; cpu_s: number; cpu_pct: number | null }[];
  pool: { jobs: number; busy_s: number; rx_dropped: number; wf_dropped: number; shed: number }[];
  wf_views: { hz: number; decim: number; fps: number; slot: number; shared: boolean; sessions: number[] }[];
  hw: { adc_overloads: number; clock_ppm: number; hw_rate_ok: boolean; rx_frames: number };
}

export type Json = null | boolean | number | string | Json[] | { [k: string]: Json };

export interface ConfigView {
  path: string | null;
  writable: boolean;
  file: { [k: string]: Json };
  running: { [k: string]: Json };
  auth: { admin_password: boolean; listener_password: boolean };
}
