# web888-ng: new server (`web888d`) for the Web-888 SDR

A Rust server for the Web-888 receiver: its own web UI, standard SDR
protocols (HPSDR, rtl_tcp, IQ), decoders, and KiwiSDR protocol
compatibility. It runs on the stock firmware (stock or open bitstream) and
later on the open driver from [web888-ng-hw](https://github.com/jeskko/web888-ng-hw).

Status: **M1 in progress** (see `doc/design.md` §11): stock-driver backend and HPSDR Protocol 1 work against the simulator and a fake driver. They haven't been tested on a unit yet.

| crate | content |
|---|---|
| `crates/radio` | `Radio` trait, sample types, channel math (NCO words, rates, CIC response) |
| `crates/sim` | simulated Web-888: RF scene → real channel model (NCO quantisation, clock error, CIC response, aliasing, noise), real-time or free-running, ring overrun and PPS |
| `crates/dsp` | Kaiser FIR, droop-compensated channel design, overlap-save filter, shifter, SSB/AM demod |
| `crates/zynqsdr` | backend for the stock kernel driver (`/dev/zynqsdr` ioctls), bitstream loading |
| `crates/hpsdr` | OpenHPSDR Protocol 1 receive server (discovery, up to 7 receivers, 48 kS/s) |
| `crates/web888d` | the daemon: `serve` (radio + HPSDR), `sim` (one channel to stdout) |

```
cargo test --workspace
cargo run --release -p web888d -- sim --tone 7.0755e6:-40 --noise --tune 7.074e6 --mode usb \
  | aplay -t raw -f FLOAT_LE -r 24000 -c 1
```

On a unit (stock firmware), with the CI binary copied over:

```
rc-service sdrd stop      # the driver allows one opener
./web888d serve           # loads websdr_hf.bit, serves HPSDR P1 on UDP 1024
```

Then point an HPSDR client (SparkSDR, SDR++ hermes source, Thetis, ...) at
the unit at 48 kS/s. `rc-service sdrd start` brings the stock server back.

Without an SDR program, `p1probe` checks a running server: it tunes
receivers, streams for a few seconds and prints rate, sequence gaps and the
strongest tones per receiver:

```
cargo run --release -p web888-hpsdr --example p1probe -- UNIT:1024 --secs 5 7074000 14074000
```

The same works against the QEMU virtual Web-888 in web888-ng-hw
(`emu/`, stock firmware against an FPGA model): boot it, fetch the
binary in the guest with `wget http://10.0.2.2:PORT/web888d` from a host
HTTP server, run `web888d serve`, then probe `127.0.0.1:1024` (UDP
forwarded). The emulated CPU can't keep up with 12 channels at 48 kS/s,
so expect `rx: ... frames lost` warnings there. They are not a bug.

CI builds a static `armv7-unknown-linux-musleabihf` binary for the device.

## Licence

GPL-3.0-or-later (`COPYING`). Code ported from RaspSDR/server (KiwiSDR-derived)
keeps its original notices. Provenance rules are in `doc/design.md` §12.
Not affiliated with the Web-888 / RX-888 vendor.
