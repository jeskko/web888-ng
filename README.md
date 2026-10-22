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
| `crates/iq` | backend fed by real signals: wideband IQ from an SDR (SoapySDR, e.g. HackRF) or a recording, channelized like the FPGA |
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

### Real signals without a Web-888 (HackRF, IQ recordings)

`--backend iq` feeds web888d from a wideband IQ stream instead of the
Web-888's ADC: the same channel structure as the FPGA (NCO, CIC chains,
rates, levels, overrun accounting), but only within the input's band. The
input rate must be 122.88 MHz / 8, 16, ... (7.68 MS/s: channels within
+-3.46 MHz of the centre; 15.36 MS/s covers 40 to 20 m at twice the CPU).

```
# live (needs libSoapySDR): 40 m + 30 m around 8.6 MHz
cargo run --release -p web888d --features soapy -- serve --backend iq \
  --iq soapy:driver=hackrf --iq-freq 8.6e6 --iq-bw 7e6 \
  --iq-gain LNA=16 --iq-gain VGA=20 --iq-gain AMP=0

# record, then replay (looping) without the device
hackrf_transfer -r hf.cs8 -f 8600000 -s 7680000 -b 7000000 -l 16 -g 20 -a 0 -n 230400000
cargo run --release -p web888d -- serve --backend iq --iq file:hf.cs8 --iq-freq 8.6e6 --iq-loop
```

Formats: `.cs8` (hackrf_transfer), `.cs16`, `.cf32`. On an HF antenna,
keep the HackRF's amplifier off and watch for overload (input samples at
full scale count as ADC overflows: HPSDR overload bit, `hw_stats`). About
6 % of one core per active channel at 7.68 MS/s
(`cargo run --release -p web888-iq --example bench 7`).

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
forwarded). Tones arrive at the model's levels and web888d reads the
full 48 kS/s.

CI builds a static `armv7-unknown-linux-musleabihf` binary for the device.

## Licence

GPL-3.0-or-later (`COPYING`). Code ported from RaspSDR/server (KiwiSDR-derived)
keeps its original notices. Provenance rules are in `doc/design.md` §12.
Not affiliated with the Web-888 / RX-888 vendor.
