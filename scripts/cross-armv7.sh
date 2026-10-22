#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Build a pure-Rust target for the Web-888 (armv7, static musl) without a
# local rustup: the official Rust image with the musl target, linked by
# rust-lld. Good for benches and tools (e.g. the DSP bench); web888d itself
# needs a C cross compiler for ring, which CI gets from `cross`.
#
#   scripts/cross-armv7.sh OUTDIR -p web888-dsp --example bench
#
# The binary lands in OUTDIR/target/armv7-unknown-linux-musleabihf/release/
# (examples/ for examples); the target dir is kept for incremental builds.
set -eu
[ $# -ge 2 ] || { echo "usage: $0 OUTDIR CARGO-BUILD-ARGS..." >&2; exit 2; }
out=$(cd "$1" && pwd)
shift
src=$(cd "$(dirname "$0")/.." && pwd)
engine=$(command -v podman || command -v docker)
exec "$engine" run --rm -v "$src:/src:ro" -v "$out:/out" \
    -e CARGO_TARGET_DIR=/out/target \
    -e CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER=rust-lld \
    docker.io/library/rust:1-slim sh -c '
        rustup target add armv7-unknown-linux-musleabihf >/dev/null 2>&1
        cd /src && cargo build --release --target armv7-unknown-linux-musleabihf "$@"' sh "$@"
