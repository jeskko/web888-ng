// SPDX-License-Identifier: GPL-3.0-or-later
// The WebSocket to web888d: JSON messages and binary frames, reconnect
// with backoff (not when the receiver is busy), ping for latency.

import { parseFrame, PROTO, type AudioFrame, type ClientMsg, type ServerMsg, type WfLine } from "./proto.ts";

export type ConnState = "connecting" | "open" | "closed" | "busy";

export interface ConnHandlers {
  onState(s: ConnState, why?: string): void;
  onMessage(m: ServerMsg): void;
  onAudio(f: AudioFrame): void;
  onWf(l: WfLine): void;
  onLatency(ms: number): void;
}

export class Conn {
  private ws: WebSocket | null = null;
  private retry = 0;
  private timer: number | undefined;
  private pinger: number | undefined;
  private busy = false;
  private url: string;
  private h: ConnHandlers;

  constructor(url: string, h: ConnHandlers) {
    this.url = url;
    this.h = h;
  }

  connect(): void {
    clearTimeout(this.timer);
    this.busy = false;
    this.h.onState("connecting");
    const ws = new WebSocket(this.url);
    ws.binaryType = "arraybuffer";
    this.ws = ws;
    ws.onopen = () => {
      this.retry = 0;
      this.h.onState("open");
      this.send({ type: "hello", proto: PROTO, codecs: ["pcm16"] });
      this.pinger = window.setInterval(() => this.send({ type: "ping", t: performance.now() }), 5000);
    };
    ws.onmessage = (e) => {
      if (typeof e.data === "string") {
        const m = JSON.parse(e.data) as ServerMsg;
        if (m.type === "busy") this.busy = true;
        if (m.type === "pong") this.h.onLatency(performance.now() - m.t);
        this.h.onMessage(m);
        return;
      }
      const f = parseFrame(e.data as ArrayBuffer);
      if (f?.kind === 1) this.h.onAudio(f);
      else if (f?.kind === 2) this.h.onWf(f);
    };
    ws.onclose = (e) => {
      clearInterval(this.pinger);
      if (this.ws !== ws) return;
      this.ws = null;
      if (this.busy || e.code === 1013) {
        this.h.onState("busy");
        return;
      }
      const delay = [1000, 2000, 5000, 10000][Math.min(this.retry++, 3)];
      this.h.onState("closed", `reconnecting in ${delay / 1000} s`);
      this.timer = window.setTimeout(() => this.connect(), delay);
    };
  }

  send(m: ClientMsg): void {
    if (this.ws?.readyState === WebSocket.OPEN) this.ws.send(JSON.stringify(m));
  }
}

/** ws:// or wss:// URL of /ws next to the page. */
export function wsUrl(loc: Location): string {
  const proto = loc.protocol === "https:" ? "wss:" : "ws:";
  const base = loc.pathname.replace(/[^/]*$/, "");
  return `${proto}//${loc.host}${base}ws`;
}
