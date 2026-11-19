#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""web888 reference client: the listener protocol of doc/api.md, from Python.

Standard library only (Python 3.8+), one file: copy it anywhere.

    web888_client.py info URL
    web888_client.py audio URL --hz 7074000 --mode usb --seconds 60 --out ft8.wav
    web888_client.py audio URL --hz 77500 --mode iq --seconds 60 --sigmf dcf77
    web888_client.py wide URL --hz 7030000 --seconds 30 --sigmf cw40
    web888_client.py waterfall URL --center 7100000 --decim 64 --seconds 10 > wf.jsonl
    web888_client.py spots URL > spots.jsonl

URL is the receiver's page (http://host:8073/) or its WebSocket
(ws://host:8073/ws). An API token (doc/api.md §8.3) goes in --token or
the WEB888_TOKEN environment variable.

It is also the test of doc/api.md: it uses only what the document says.
"""

import argparse
import base64
import datetime
import hashlib
import json
import os
import socket
import ssl
import struct
import sys
import time
import urllib.parse
import urllib.request
import wave

PROTO = 3
CLIENT = "web888-client 0.1"

# binary frame kinds and flags (doc/api.md §6)
KIND_AUDIO, KIND_WF, KIND_IQ = 1, 2, 3
F_OVERLOAD, F_GAP, F_STEREO, F_MULAW, F_OPUS, F_TIME = 1, 2, 4, 8, 16, 32
F_WF_PACKED = 8
F_IQ_I16 = 4

CLOSE_WHY = {
    1000: "closed",
    1008: "kicked by the admin, or the token revoked",
    1013: "refused at admission",
    4001: "session time limit",
    4002: "idle limit",
    4003: "daily minutes used",
    4004: "protocol version not supported",
}


def log(*a):
    print(*a, file=sys.stderr, flush=True)


# --- a small WebSocket client (RFC 6455): what the protocol needs -------


class Closed(Exception):
    def __init__(self, code, reason):
        super().__init__(f"{code} {CLOSE_WHY.get(code, '')} {reason}".strip())
        self.code = code
        self.reason = reason


class WebSocket:
    def __init__(self, url, headers=None, timeout=20.0):
        u = urllib.parse.urlsplit(url)
        secure = u.scheme == "wss"
        port = u.port or (443 if secure else 80)
        raw = socket.create_connection((u.hostname, port), timeout=timeout)
        if secure:
            raw = ssl.create_default_context().wrap_socket(raw, server_hostname=u.hostname)
        self.sock = raw
        self.buf = b""
        key = base64.b64encode(os.urandom(16)).decode()
        host = u.hostname if u.port is None else f"{u.hostname}:{u.port}"
        req = [
            f"GET {u.path or '/'}{'?' + u.query if u.query else ''} HTTP/1.1",
            f"Host: {host}",
            "Upgrade: websocket",
            "Connection: Upgrade",
            f"Sec-WebSocket-Key: {key}",
            "Sec-WebSocket-Version: 13",
            f"User-Agent: {CLIENT}",
        ]
        req += [f"{k}: {v}" for k, v in (headers or {}).items()]
        self.sock.sendall(("\r\n".join(req) + "\r\n\r\n").encode())
        head = self._until(b"\r\n\r\n").decode("latin-1")
        status = head.split("\r\n", 1)[0]
        if " 101 " not in status + " ":
            body = self.sock.recv(4096).decode("utf-8", "replace").strip()
            raise ConnectionError(f"{status}: {body}" if body else status)
        accept = base64.b64encode(
            hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()
        ).decode()
        if accept not in head:
            raise ConnectionError("bad WebSocket handshake")

    def _read(self, n):
        while len(self.buf) < n:
            d = self.sock.recv(65536)
            if not d:
                raise Closed(1006, "connection lost")
            self.buf += d
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def _until(self, sep):
        while sep not in self.buf:
            d = self.sock.recv(65536)
            if not d:
                raise ConnectionError("connection closed during the handshake")
            self.buf += d
        i = self.buf.index(sep) + len(sep)
        out, self.buf = self.buf[:i], self.buf[i:]
        return out

    def _send(self, op, data):
        n = len(data)
        head = bytes([0x80 | op])
        if n < 126:
            head += bytes([0x80 | n])
        elif n < 65536:
            head += bytes([0x80 | 126]) + struct.pack(">H", n)
        else:
            head += bytes([0x80 | 127]) + struct.pack(">Q", n)
        mask = os.urandom(4)
        body = bytes(b ^ mask[i & 3] for i, b in enumerate(data))
        self.sock.sendall(head + mask + body)

    def send(self, msg):
        self._send(1, json.dumps(msg).encode())

    def recv(self):
        """The next message: str (text) or bytes (binary)."""
        parts, kind = [], None
        while True:
            b0, b1 = self._read(2)
            op, n = b0 & 0x0F, b1 & 0x7F
            if n == 126:
                (n,) = struct.unpack(">H", self._read(2))
            elif n == 127:
                (n,) = struct.unpack(">Q", self._read(8))
            mask = self._read(4) if b1 & 0x80 else None
            data = self._read(n)
            if mask:
                data = bytes(b ^ mask[i & 3] for i, b in enumerate(data))
            if op == 8:  # close
                code = struct.unpack(">H", data[:2])[0] if len(data) >= 2 else 1005
                try:
                    self._send(8, data[:2])
                except OSError:
                    pass
                raise Closed(code, data[2:].decode("utf-8", "replace"))
            if op == 9:  # ping
                self._send(10, data)
                continue
            if op == 10:
                continue
            if op in (1, 2):
                kind, parts = op, [data]
            elif op == 0:
                parts.append(data)
            if b0 & 0x80:
                d = b"".join(parts)
                return d.decode() if kind == 1 else d

    def close(self):
        try:
            self._send(8, struct.pack(">H", 1000))
            self.sock.close()
        except OSError:
            pass


# --- frames (doc/api.md §6) ---------------------------------------------


def mulaw(b):
    out = []
    for u in b:
        u = ~u & 0xFF
        t = (((u & 0x0F) << 3) + 0x84) << ((u & 0x70) >> 4)
        out.append((0x84 - t) if u & 0x80 else (t - 0x84))
    return out


def parse_audio(b):
    flags = b[1]
    (count, seq, rate, power) = struct.unpack_from("<HIIf", b, 2)
    at = 16
    t = None
    if flags & F_TIME:
        (t,) = struct.unpack_from("<d", b, 16)
        at = 24
    if flags & F_OPUS:
        pcm = None  # one Opus packet: this client asks for PCM
    elif flags & F_MULAW:
        pcm = mulaw(b[at : at + count])
    else:
        pcm = list(struct.unpack_from(f"<{count}h", b, at))
    return {
        "flags": flags,
        "count": count,
        "seq": seq,
        "rate": rate,
        "power_dbfs": power,
        "time": t,
        "stereo": bool(flags & F_STEREO),
        "pcm": pcm,
    }


def parse_wf(b):
    flags = b[1]
    (bins, seq, f0, df, db0, step, t_ms) = struct.unpack_from("<HIddffd", b, 2)
    if flags & F_WF_PACKED:
        raise ValueError("packed line: this client doesn't ask for them")
    lv = b[40 : 40 + bins]
    return {
        "flags": flags,
        "seq": seq,
        "t": t_ms / 1000.0,
        "f0": f0,
        "df": df,
        "db": [round(db0 + v * step, 1) for v in lv],
    }


def parse_iq(b):
    flags = b[1]
    (count, seq, frame, centre, rate, scale) = struct.unpack_from("<HIQdff", b, 2)
    at, t = 32, None
    if flags & F_TIME:
        (t,) = struct.unpack_from("<d", b, 32)
        at = 40
    if flags & F_IQ_I16:
        v = struct.unpack_from(f"<{2 * count}h", b, at)
    else:
        v = struct.unpack_from(f"<{2 * count}b", b, at)
    return {
        "flags": flags,
        "count": count,
        "seq": seq,
        "frame": frame,
        "centre": centre,
        "rate": rate,
        "time": t,
        "iq": [x * scale for x in v],
    }


# --- a session ------------------------------------------------------------


def ws_url(url):
    u = urllib.parse.urlsplit(url)
    scheme = {"http": "ws", "https": "wss"}.get(u.scheme, u.scheme)
    path = u.path if u.path.endswith("/ws") else u.path.rstrip("/") + "/ws"
    return urllib.parse.urlunsplit((scheme, u.netloc, path, "", ""))


def http_url(url, path):
    u = urllib.parse.urlsplit(url)
    scheme = {"ws": "http", "wss": "https"}.get(u.scheme, u.scheme)
    return urllib.parse.urlunsplit((scheme, u.netloc, path, "", ""))


class Session:
    """One /ws session: hello, state and lease read, the client's hello sent."""

    def __init__(self, url, token=None):
        headers = {"Authorization": f"Bearer {token}"} if token else {}
        self.ws = WebSocket(ws_url(url), headers)
        self.hello = self.state = self.lease = self.timebase = None
        self.next_id = 1
        while self.hello is None or self.state is None or self.lease is None:
            self._handle(self.ws.recv())
        if self.hello.get("proto", 2) < PROTO:
            log(f"server speaks protocol {self.hello.get('proto')}: asking for it")
        self.proto = min(PROTO, self.hello.get("proto", 2))
        self.ws.send({"type": "hello", "proto": self.proto, "codecs": ["pcm16"]})

    def has(self, feature):
        return feature in self.hello.get("features", [])

    def send(self, msg):
        """Send a request; with protocol 3 it gets an id, which its reply echoes."""
        rid = None
        if self.proto >= 3:
            rid = self.next_id
            msg = dict(msg, id=rid)
            self.next_id += 1
        self.ws.send(msg)
        return rid

    def _handle(self, m):
        """Keep the session's view; return the message (dict or bytes)."""
        if isinstance(m, bytes):
            return m
        m = json.loads(m)
        t = m.get("type")
        if t == "hello":
            self.hello = m
        elif t == "state":
            self.state = m
        elif t == "lease":
            if self.lease is not None and m != self.lease:
                if m.get("granted"):
                    log("receiver granted")
                else:
                    log(f"waiting for a receiver: {m.get('position')} in the queue")
            self.lease = m
        elif t == "timebase":
            self.timebase = m
        elif t == "busy":
            log(f"busy: {m.get('msg')}")
        elif t == "error":
            log(f"error: {m.get('msg')}" + (f" (request {m['id']})" if "id" in m else ""))
        return m

    def messages(self, seconds=None):
        """Messages until `seconds` have passed (None: until closed)."""
        end = None if seconds is None else time.monotonic() + seconds
        while end is None or time.monotonic() < end:
            self.ws.sock.settimeout(None if end is None else max(0.1, end - time.monotonic()))
            try:
                m = self.ws.recv()
            except socket.timeout:
                return
            yield self._handle(m)

    def wait_state(self, rid, seconds=5):
        """The state answering request `rid` (protocol 2: the next state)."""
        for m in self.messages(seconds):
            if isinstance(m, dict) and m.get("type") == "state" and m.get("id") == rid:
                return m
        return self.state

    def close(self):
        self.ws.close()


# --- SigMF (https://sigmf.org) ----------------------------------------------


def iso(t):
    d = datetime.datetime.fromtimestamp(t, datetime.timezone.utc)
    return d.strftime("%Y-%m-%dT%H:%M:%S.%f") + "Z"


class SigMF:
    """A recording: base.sigmf-data and base.sigmf-meta. A capture
    starts at the first sample and after each gap or jump in time, with
    the antenna time of its first sample when the server gives times."""

    def __init__(self, base, datatype, rate, description, hw, channels=1):
        self.base = base
        self.channels = channels
        self.data = open(base + ".sigmf-data", "wb")
        self.datatype = datatype
        self.rate = rate
        self.n = 0
        self.captures = []
        self.description = description
        self.hw = hw
        self.next_t = None
        self.timebase = None

    def write(self, samples, packed, freq, t, gap):
        """`samples` items (complex or real) as `packed` bytes; `t`: the
        first one's UTC or None."""
        jump = t is not None and self.next_t is not None and abs(t - self.next_t) > 0.002
        last = self.captures[-1] if self.captures else None
        if last is None or gap or jump or last["core:frequency"] != freq:
            c = {"core:sample_start": self.n, "core:frequency": freq}
            if t is not None:
                c["core:datetime"] = iso(t)
            self.captures.append(c)
        self.data.write(packed)
        self.n += samples
        self.next_t = None if t is None else t + samples / self.rate

    def close(self):
        self.data.close()
        g = {
            "core:datatype": self.datatype,
            "core:sample_rate": self.rate,
            "core:version": "1.2.0",
            "core:num_channels": self.channels,
            "core:recorder": CLIENT,
            "core:hw": self.hw,
            "core:description": self.description,
        }
        if self.timebase:
            g["core:extensions"] = [{"name": "web888", "version": "0.1.0", "optional": True}]
            tb = {"source": self.timebase.get("source")}
            for k in ("uncertainty", "adc_ppm"):
                if k in self.timebase:
                    tb[k] = self.timebase[k]
            g["web888:timebase"] = tb
        meta = {"global": g, "captures": self.captures, "annotations": []}
        with open(self.base + ".sigmf-meta", "w") as f:
            json.dump(meta, f, indent=2)
            f.write("\n")


# --- commands -----------------------------------------------------------------


def clock(tb):
    out = tb.get("source", "?")
    if "uncertainty" in tb:
        out += f" ±{tb['uncertainty'] * 1e3:.1f} ms"
    if "adc_ppm" in tb:
        out += f", ADC {tb['adc_ppm']:+.2f} ppm"
    return out


def hw_name(s):
    r = s.hello.get("radio", {})
    return f"{s.hello.get('server', 'web888d')} ({r.get('backend', '?')}, {r.get('variant', '?')})"


def cmd_info(a):
    try:
        req = urllib.request.Request(http_url(a.url, "/api/version"), headers={"User-Agent": CLIENT})
        with urllib.request.urlopen(req, timeout=10) as r:
            version = json.load(r)
    except Exception as e:  # an older server has no /api/version
        version = {"error": str(e)}
    s = Session(a.url, a.token)
    h = s.hello
    out = {
        "version": version,
        "proto": h.get("proto"),
        "server": h.get("server"),
        "class": h.get("class"),
        "radio": h.get("radio"),
        "codecs": h.get("codecs"),
        "audio_rates": h.get("audio_rates"),
        "features": h.get("features", []),
        "state": {k: s.state.get(k) for k in ("hz", "mode", "lo", "hi", "audio_rate")},
        "lease": s.lease,
    }
    s.close()
    print(json.dumps(out, indent=2))


def wait_lease(s, seconds=60):
    if s.lease.get("granted"):
        return True
    log(f"waiting for a receiver: {s.lease.get('position')} in the queue")
    for _ in s.messages(seconds):
        if s.lease.get("granted"):
            return True
    return False


def cmd_audio(a):
    if not a.out and not a.sigmf:
        sys.exit("audio: --out FILE.wav or --sigmf BASE")
    s = Session(a.url, a.token)
    s.send({"type": "mode", "mode": a.mode, **({"lo": a.lo} if a.lo is not None else {}),
            **({"hi": a.hi} if a.hi is not None else {})})
    s.send({"type": "tune", "hz": a.hz})
    audio = {"type": "audio", "codec": "pcm16", "agc": not a.no_agc}
    if a.rate:
        audio["audio_rate"] = a.rate
    if a.gain is not None:
        audio["gain_db"] = a.gain
    timed = s.has("timestamps")
    if timed:
        audio["timestamps"] = True
    st = s.wait_state(s.send(audio))
    log(f"{st['hz']:.0f} Hz {st['mode']} {st['lo']:.0f}..{st['hi']:.0f} Hz, "
        f"{st['audio_rate']} S/s{', timed' if timed else ''}")
    if not wait_lease(s):
        sys.exit("no receiver within a minute")
    wav = rec = None
    channels = 2 if st["mode"] in ("iq", "sas") else 1
    frames = gaps = 0
    t0 = None
    for m in s.messages(a.seconds):
        if not isinstance(m, bytes) or m[0] != KIND_AUDIO:
            continue
        f = parse_audio(m)
        if f["pcm"] is None:
            continue
        if f["flags"] & F_GAP and frames:
            gaps += 1
        if t0 is None:
            t0 = f["time"]
        packed = struct.pack(f"<{len(f['pcm'])}h", *f["pcm"])
        if a.out and wav is None:
            wav = wave.open(a.out, "wb")
            wav.setnchannels(2 if f["stereo"] else 1)
            wav.setsampwidth(2)
            wav.setframerate(f["rate"])
        if wav:
            wav.writeframes(packed)
        if a.sigmf:
            if rec is None:
                iq = st["mode"] == "iq"
                rec = SigMF(a.sigmf, "ci16_le" if iq else "ri16_le", f["rate"],
                            f"web888 audio, {st['mode']} {st['lo']:.0f}..{st['hi']:.0f} Hz at the dial",
                            hw_name(s), 1 if iq else channels)
                if channels == 2 and not iq:
                    rec.description += " (stereo: lower sideband left, upper right)"
            n = len(f["pcm"]) // (2 if f["stereo"] else 1)
            rec.write(n, packed, s.state["hz"], f["time"], f["flags"] & F_GAP)
        frames += 1
    if wav:
        wav.close()
    if rec:
        rec.timebase = s.timebase
        rec.close()
    s.close()
    log(f"{frames} frames, {gaps} gaps" + (f", first sample at {iso(t0)}" if t0 else "")
        + (f"; clock: {clock(s.timebase)}" if s.timebase else ""))


def cmd_wide(a):
    s = Session(a.url, a.token)
    if not s.hello.get("wide"):
        sys.exit("this receiver has no wide IQ (no channelizer bins)")
    timed = s.has("timestamps")
    s.send({"type": "wide", "on": True, "hz": a.hz, "bits": 16, **({"timestamps": True} if timed else {})})
    if timed:
        # the timebase messages come with audio timestamps on
        s.send({"type": "audio", "timestamps": True})
    rec = None
    expect = None
    frames = gaps = 0
    for m in s.messages(a.seconds):
        if not isinstance(m, bytes) or m[0] != KIND_IQ:
            continue
        f = parse_iq(m)
        gap = expect is not None and (bool(f["flags"] & F_GAP) or f["frame"] != expect)
        gaps += gap
        expect = f["frame"] + f["count"]
        if rec is None:
            rec = SigMF(a.sigmf, "cf32_le", f["rate"],
                        f"web888 wide IQ: one channelizer bin, clean within ±21 kHz of its centre", hw_name(s))
            log(f"bin at {f['centre']:.0f} Hz, {f['rate']:.0f} S/s{', timed' if f['time'] else ''}")
        rec.write(f["count"], struct.pack(f"<{len(f['iq'])}f", *f["iq"]), f["centre"], f["time"], gap)
        frames += 1
    s.send({"type": "wide", "on": False})
    if rec:
        rec.timebase = s.timebase
        rec.close()
    s.close()
    log(f"{frames} frames, {gaps} gaps" + (f"; clock: {clock(s.timebase)}" if s.timebase else ""))


def cmd_waterfall(a):
    s = Session(a.url, a.token)
    wf = {"type": "wf", "bins": a.bins, "fps": a.fps}
    if a.center is not None:
        wf["center_hz"] = a.center
    if a.decim is not None:
        wf["decim"] = a.decim
    log(f"view {s.wait_state(s.send(wf))['wf']}")
    for m in s.messages(a.seconds):
        if isinstance(m, bytes) and m[0] == KIND_WF:
            print(json.dumps(parse_wf(m)), flush=True)
    s.close()


def cmd_spots(a):
    s = Session(a.url, a.token)
    if not s.hello.get("spots"):
        sys.exit("this receiver runs no skimmers")
    s.send({"type": "spots", "on": True})
    for m in s.messages(a.seconds):
        if isinstance(m, dict) and m.get("type") == "spots":
            for x in m["spots"]:
                print(json.dumps(x), flush=True)
    s.close()


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--token", default=os.environ.get("WEB888_TOKEN"), help="API token (or WEB888_TOKEN)")
    sub = p.add_subparsers(dest="cmd", required=True)

    c = sub.add_parser("info", help="the server's version, features, receiver")
    c.add_argument("url")
    c.set_defaults(fn=cmd_info)

    c = sub.add_parser("audio", help="record audio: WAV and/or SigMF")
    c.add_argument("url")
    c.add_argument("--hz", type=float, required=True, help="dial (Hz)")
    c.add_argument("--mode", default="usb")
    c.add_argument("--lo", type=float, help="passband low edge (Hz from the dial)")
    c.add_argument("--hi", type=float, help="passband high edge (Hz from the dial)")
    c.add_argument("--rate", type=int, help="audio rate (one of hello.audio_rates)")
    c.add_argument("--gain", type=float, help="gain dB (maximum with AGC)")
    c.add_argument("--no-agc", action="store_true")
    c.add_argument("--seconds", type=float, default=10)
    c.add_argument("--out", help="WAV file")
    c.add_argument("--sigmf", help="SigMF recording BASE(.sigmf-data, .sigmf-meta), with antenna times")
    c.set_defaults(fn=cmd_audio)

    c = sub.add_parser("wide", help="record one channelizer bin as SigMF (cf32)")
    c.add_argument("url")
    c.add_argument("--hz", type=float, required=True, help="the bin nearest this (Hz)")
    c.add_argument("--seconds", type=float, default=10)
    c.add_argument("--sigmf", required=True, help="recording BASE")
    c.set_defaults(fn=cmd_wide)

    c = sub.add_parser("waterfall", help="waterfall lines as JSON lines (absolute dB)")
    c.add_argument("url")
    c.add_argument("--center", type=float, help="centre (Hz)")
    c.add_argument("--decim", type=int, help="span = adc_hz / decim")
    c.add_argument("--bins", type=int, default=1024)
    c.add_argument("--fps", type=int, default=1)
    c.add_argument("--seconds", type=float, default=10)
    c.set_defaults(fn=cmd_waterfall)

    c = sub.add_parser("spots", help="the skimmers' spots as JSON lines")
    c.add_argument("url")
    c.add_argument("--seconds", type=float, help="stop after (default: follow)")
    c.set_defaults(fn=cmd_spots)

    a = p.parse_args()
    try:
        a.fn(a)
    except Closed as e:
        sys.exit(f"closed: {e}")
    except ConnectionError as e:
        sys.exit(f"refused: {e}")
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
