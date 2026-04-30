#!/bin/bash
# Build bastion-worker as a static MUSL binary
set -e

echo "Building bastion-worker (static MUSL binary)..."

# Install target if needed
rustup target add x86_64-unknown-linux-musl 2>/dev/null || true

# Build
cargo build --release --target x86_64-unknown-linux-musl -p bastion-worker

BINARY="target/x86_64-unknown-linux-musl/release/bastion-worker"
SIZE=$(du -h "$BINARY" | cut -f1)

echo "Built: $BINARY ($SIZE)"
file "$BINARY"
