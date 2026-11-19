#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# The reference client against a simulated receiver (CI): info, audio as
# WAV and SigMF (with antenna times), IQ audio, waterfall lines, and wide
# IQ from the iq backend's bins (a generated tone file).
#   tools/web888-client/test.sh [path/to/web888d]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin=${1:-target/release/web888d}
t=$(mktemp -d)
pids=""
trap 'for p in $pids; do kill "$p" 2>/dev/null || true; done; rm -rf "$t"' EXIT
: > "$t/c.toml"
client() { python3 -I "$here/web888_client.py" "$@"; }

serve() { # PORT ARGS...: start web888d, wait for its port
	port=$1
	shift
	"$bin" serve --config "$t/c.toml" --no-hpsdr --no-tls --http "127.0.0.1:$port" \
		--state-dir "$t" --ui-dir "$t" "$@" > "$t/server-$port.log" 2>&1 &
	pids="$pids $!"
	for _ in $(seq 100); do
		python3 -I -c "import socket; socket.create_connection(('127.0.0.1', $port), 1)" 2>/dev/null && return 0
		sleep 0.2
	done
	cat "$t/server-$port.log"
	exit 1
}

serve 28190 --backend sim --noise --tone 7075000:-40
u=http://127.0.0.1:28190/

client info $u > "$t/info.json"
client audio $u --hz 7074000 --mode usb --seconds 3 --out "$t/a.wav" --sigmf "$t/a"
client audio $u --hz 7074000 --mode iq --seconds 2 --sigmf "$t/iq"
client waterfall $u --center 7074000 --decim 4096 --bins 1024 --fps 4 --seconds 2 > "$t/wf.jsonl"

# wide IQ: 0.5 s of a tone 10 kHz above 7.04 MHz, 1.92 MS/s cs8, looped
python3 -I - "$t/tone.cs8" <<'EOF'
import math, sys
rate, f = 1_920_000, 7_050_000 - 7_040_000
with open(sys.argv[1], "wb") as o:
    o.write(bytes(
        b & 0xFF
        for n in range(rate // 2)
        for b in (round(60 * math.cos(2 * math.pi * f * n / rate)), round(60 * math.sin(2 * math.pi * f * n / rate)))
    ))
EOF
serve 28191 --backend iq --iq "file:$t/tone.cs8" --iq-freq 7040000 --iq-rate 1920000 --iq-loop --bins 16
client wide http://127.0.0.1:28191/ --hz 7050000 --seconds 3 --sigmf "$t/wide"

python3 -I - "$t" <<'EOF'
import cmath, datetime, json, math, struct, sys, time, wave
t = sys.argv[1]
now = time.time()

def meta(name):
    return json.load(open(f"{t}/{name}.sigmf-meta"))

def when(c):
    return datetime.datetime.strptime(c["core:datetime"], "%Y-%m-%dT%H:%M:%S.%fZ") \
        .replace(tzinfo=datetime.timezone.utc).timestamp()

def tone(c, rate):
    p = sum(c[i] * c[i - 1].conjugate() for i in range(1, len(c)))
    return cmath.phase(p) / (2 * math.pi) * rate

info = json.load(open(f"{t}/info.json"))
assert info["proto"] == 3 and "timestamps" in info["features"], info

w = wave.open(f"{t}/a.wav")
n, r = w.getnframes(), w.getframerate()
assert r == 12000 and 2.5 * r < n < 3.5 * r, (n, r)
x = struct.unpack(f"<{n}h", w.readframes(n))
z = sum(1 for i in range(1, n) if x[i - 1] < 0 <= x[i]) / (n / r)
assert abs(z - 1000) < 20, f"USB tone at {z} Hz"

m = meta("a")
g, c = m["global"], m["captures"][0]
assert g["core:datatype"] == "ri16_le" and g["core:sample_rate"] == 12000, g
assert c["core:frequency"] == 7074000 and abs(when(c) - now) < 30, c
assert g["web888:timebase"]["source"] in ("ntp", "holdover", "none"), g

m = meta("iq")
assert m["global"]["core:datatype"] == "ci16_le", m
d = open(f"{t}/iq.sigmf-data", "rb").read()
v = struct.unpack(f"<{len(d) // 2}h", d)
f = tone([complex(v[2 * i], v[2 * i + 1]) for i in range(len(v) // 2)], 12000)
assert abs(f - 1000) < 20, f"IQ tone at {f} Hz"

lines = [json.loads(l) for l in open(f"{t}/wf.jsonl")]
assert len(lines) >= 4, len(lines)
peak = max(range(len(lines[-1]["db"])), key=lambda k: lines[-1]["db"][k])
hz = lines[-1]["f0"] + peak * lines[-1]["df"]
assert abs(hz - 7075000) < 100 and abs(lines[-1]["t"] - now) < 30, (hz, lines[-1]["t"])

m = meta("wide")
g, cs = m["global"], m["captures"]
assert g["core:datatype"] == "cf32_le" and g["core:sample_rate"] == 60000, g
assert cs[0]["core:frequency"] == 7040000 and abs(when(cs[0]) - now) < 30, cs
d = open(f"{t}/wide.sigmf-data", "rb").read()
v = struct.unpack(f"<{len(d) // 4}f", d)
assert len(v) // 2 > 2 * 60000, len(v)
f = tone([complex(v[2 * i], v[2 * i + 1]) for i in range(len(v) // 2)][-60000:], 60000)
assert abs(f - 10000) < 50, f"wide tone at {f} Hz"
print("web888-client: all good")
EOF
