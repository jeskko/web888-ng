#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# The web UI's WebAssembly, to ui/wasm/, which `pnpm build` copies into
# dist/: the JTTY receiver (crates/jtty-wasm) as jtty.wasm, and the FT8,
# FT4 and WSPR decoders (crates/digi-wasm) as digi.wasm, the CW skimmer
# (crates/cw-wasm) as cw.wasm, the FSK/RTTY skimmer (crates/fsk-wasm) as
# fsk.wasm, the PSK31/63/125 skimmer (crates/psk-wasm) as psk.wasm, the
# HFDL decoder (crates/hfdl-wasm) as hfdl.wasm, the JS8 decoder
# (crates/js8-wasm) as js8.wasm, the
# fax and SSTV decoders (crates/img-wasm) as img.wasm, the signal
# identifier (crates/sigid-wasm) as sigid.wasm and the time-code receiver
# (crates/timecode-wasm) as timecode.wasm.
#
#   scripts/build-wasm.sh
#
# With the wasm32-unknown-unknown target installed (CI: rustup) it builds
# locally; otherwise in the official Rust image (podman or docker), as
# cross-armv7.sh does.
set -eu
src=$(cd "$(dirname "$0")/.." && pwd)
target=wasm32-unknown-unknown
out=$src/ui/wasm
# WebAssembly SIMD (128-bit): every browser since 2021-2023 (Safari 16.4);
# the neural CW decoder's dot products vectorise with it
export CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="-C target-feature=+simd128"
crates="-p web888-jtty-wasm -p web888-digi-wasm -p web888-cw-wasm -p web888-fsk-wasm -p web888-psk-wasm -p web888-hfdl-wasm -p web888-js8-wasm -p web888-img-wasm -p web888-sigid-wasm -p web888-timecode-wasm"
mkdir -p "$out"
if [ -d "$(rustc --print sysroot 2>/dev/null)/lib/rustlib/$target" ]; then
	(cd "$src" && cargo build --release --target $target $crates)
	built=$src/target/$target/release
else
	engine=$(command -v podman || command -v docker)
	mkdir -p "$src/target/wasm-build"
	"$engine" run --rm -v "$src:/src:ro" -v "$src/target/wasm-build:/out" \
		-e CARGO_TARGET_DIR=/out/target \
		-e CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS \
		docker.io/library/rust:1-slim sh -c "
			rustup target add $target >/dev/null 2>&1
			cd /src && cargo build --release --target $target $crates"
	built=$src/target/wasm-build/target/$target/release
fi
cp "$built/web888_jtty_wasm.wasm" "$out/jtty.wasm"
cp "$built/web888_digi_wasm.wasm" "$out/digi.wasm"
cp "$built/web888_cw_wasm.wasm" "$out/cw.wasm"
cp "$built/web888_fsk_wasm.wasm" "$out/fsk.wasm"
cp "$built/web888_psk_wasm.wasm" "$out/psk.wasm"
cp "$built/web888_hfdl_wasm.wasm" "$out/hfdl.wasm"
cp "$built/web888_js8_wasm.wasm" "$out/js8.wasm"
cp "$built/web888_img_wasm.wasm" "$out/img.wasm"
cp "$built/web888_sigid_wasm.wasm" "$out/sigid.wasm"
cp "$built/web888_timecode_wasm.wasm" "$out/timecode.wasm"
ls -l "$out/jtty.wasm" "$out/digi.wasm" "$out/cw.wasm" "$out/fsk.wasm" "$out/psk.wasm" "$out/hfdl.wasm" "$out/js8.wasm" "$out/img.wasm" "$out/sigid.wasm" "$out/timecode.wasm"
