#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# The web UI's WebAssembly: the JTTY receiver (crates/jtty-wasm) to
# ui/wasm/jtty.wasm, which `pnpm build` copies into dist/.
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
mkdir -p "$out"
if [ -d "$(rustc --print sysroot 2>/dev/null)/lib/rustlib/$target" ]; then
	(cd "$src" && cargo build --release --target $target -p web888-jtty-wasm)
	cp "$src/target/$target/release/web888_jtty_wasm.wasm" "$out/jtty.wasm"
else
	engine=$(command -v podman || command -v docker)
	mkdir -p "$src/target/wasm-build"
	"$engine" run --rm -v "$src:/src:ro" -v "$src/target/wasm-build:/out" \
		-e CARGO_TARGET_DIR=/out/target \
		docker.io/library/rust:1-slim sh -c "
			rustup target add $target >/dev/null 2>&1
			cd /src && cargo build --release --target $target -p web888-jtty-wasm"
	cp "$src/target/wasm-build/target/$target/release/web888_jtty_wasm.wasm" "$out/jtty.wasm"
fi
ls -l "$out/jtty.wasm"
