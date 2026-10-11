#!/bin/sh
# Builds the Rust static library and runs the C smoke test against the exact ABI the Swift app uses.
set -e
cd "$(dirname "$0")/.."
cargo build --release -p ppcx -p ppcx-ffi
cc tests/ffi_smoke.c -Iswift/Sources/CPPCX/include target/release/libppcx_ffi.a -lpthread -ldl -lm -o target/ffi_smoke
target/ffi_smoke "$(pwd)"
