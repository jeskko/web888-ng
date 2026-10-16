# web888-ng: new server (`web888d`) for the Web-888 SDR

A Rust server for the Web-888 receiver: its own web UI, standard SDR
protocols (HPSDR, rtl_tcp, IQ), decoders, and KiwiSDR protocol
compatibility. It runs on the stock firmware (stock or open bitstream) and
later on the open driver from [web888-ng-hw](https://github.com/jeskko/web888-ng-hw).

Status: **M0** (see `doc/design.md` §11). There's no hardware backend yet.

| crate | content |
|---|---|
| `crates/radio` | `Radio` trait, sample types, channel math (NCO words, rates, CIC response) |
| `crates/sim` | simulated Web-888: RF scene → real channel model (NCO quantisation, clock error, CIC response, aliasing, noise), real-time or free-running, ring overrun and PPS |
| `crates/dsp` | Kaiser FIR, droop-compensated channel design, overlap-save filter, shifter, SSB/AM demod |
| `crates/web888d` | the daemon; for now `web888d sim` (one channel to stdout) |

```
cargo test --workspace
cargo run --release -p web888d -- sim --tone 7.0755e6:-40 --noise --tune 7.074e6 --mode usb \
  | aplay -t raw -f FLOAT_LE -r 24000 -c 1
```

CI builds a static `armv7-unknown-linux-musleabihf` binary for the device.

## Licence

GPL-3.0-or-later (`COPYING`). Code ported from RaspSDR/server (KiwiSDR-derived)
keeps its original notices. Provenance rules are in `doc/design.md` §12.
Not affiliated with the Web-888 / RX-888 vendor.
