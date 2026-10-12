#!/usr/bin/env bash
set -e

echo "[AdityaOS] building..."
cargo build --release

echo "[AdityaOS] launching QEMU..."
qemu-system-aarch64 \
    -M virt,gic-version=2 \
    -cpu cortex-a53 \
    -m 256M \
    -nographic \
    -serial mon:stdio \
    -kernel target/aarch64-unknown-none/release/adityaos
