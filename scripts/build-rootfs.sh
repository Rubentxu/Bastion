#!/bin/bash
# Build a Firecracker rootfs with Bastion worker binary
#
# Usage: ./scripts/build-rootfs.sh <base-rootfs.img> <output-rootfs.img> [worker-binary]
#
# The base rootfs should be an ext4 image (e.g., Debian bookworm rootfs).
# The worker binary must be a static MUSL build for the target architecture.
#
# Example:
#   cargo build -p bastion-worker --release --target x86_64-unknown-linux-musl
#   ./scripts/build-rootfs.sh rootfs.ext4 bastion-rootfs.img

set -euo pipefail

BASE_ROOTFS="${1:?Usage: $0 <base-rootfs.img> <output-rootfs.img> [worker-binary]}"
OUTPUT_ROOTFS="${2:?Output rootfs path required}"
WORKER_BIN="${3:-target/x86_64-unknown-linux-musl/release/bastion-worker}"

if [ "$(id -u)" -ne 0 ]; then
    echo "Error: This script requires root (for mount -o loop)"
    echo "Run with: sudo $0 $*"
    exit 1
fi

if [ ! -f "$BASE_ROOTFS" ]; then
    echo "Error: Base rootfs not found: $BASE_ROOTFS"
    exit 1
fi

if [ ! -f "$WORKER_BIN" ]; then
    echo "Error: Worker binary not found: $WORKER_BIN"
    echo "Build it first: cargo build -p bastion-worker --release --target x86_64-unknown-linux-musl"
    exit 1
fi

echo "==> Copying base rootfs..."
cp "$BASE_ROOTFS" "$OUTPUT_ROOTFS"

MOUNT_DIR=$(mktemp -d)
echo "==> Mounting rootfs at $MOUNT_DIR..."
mount -o loop "$OUTPUT_ROOTFS" "$MOUNT_DIR"

cleanup() {
    echo "==> Unmounting..."
    umount "$MOUNT_DIR" 2>/dev/null || true
    rmdir "$MOUNT_DIR" 2>/dev/null || true
}
trap cleanup EXIT

echo "==> Injecting worker binary..."
mkdir -p "$MOUNT_DIR/usr/local/bin"
cp "$WORKER_BIN" "$MOUNT_DIR/usr/local/bin/bastion-worker"
chmod +x "$MOUNT_DIR/usr/local/bin/bastion-worker"

echo "==> Creating /workspace directory..."
mkdir -p "$MOUNT_DIR/workspace"
chmod 1777 "$MOUNT_DIR/workspace"

echo "==> Creating init script..."
cat > "$MOUNT_DIR/usr/local/bin/bastion-worker-launch.sh" << 'INIT_EOF'
#!/bin/sh
# Bastion worker launcher — reads config from kernel boot args
# Boot args format: bastion.gateway=10.0.2.1:50052 bastion.sandbox_id=xxx bastion.secret=xxx

GATEWAY=""
SANDBOX_ID=""
SECRET=""

# Parse /proc/cmdline for bastion.* parameters
for arg in $(cat /proc/cmdline); do
    case "$arg" in
        bastion.gateway=*)    GATEWAY="http://${arg#bastion.gateway=}" ;;
        bastion.sandbox_id=*) SANDBOX_ID="${arg#bastion.sandbox_id=}" ;;
        bastion.secret=*)     SECRET="${arg#bastion.secret=}" ;;
    esac
done

if [ -z "$GATEWAY" ] || [ -z "$SANDBOX_ID" ]; then
    echo "bastion-worker: Missing boot args (gateway=$GATEWAY, sandbox_id=$SANDBOX_ID)"
    exit 1
fi

echo "bastion-worker: Starting (gateway=$GATEWAY, sandbox=$SANDBOX_ID)"
exec /usr/local/bin/bastion-worker \
    --gateway-addr "$GATEWAY" \
    --sandbox-id "$SANDBOX_ID" \
    ${SECRET:+--secret "$SECRET"} \
    --workdir /workspace
INIT_EOF
chmod +x "$MOUNT_DIR/usr/local/bin/bastion-worker-launch.sh"

echo "==> Rootfs built successfully: $OUTPUT_ROOTFS"
echo "    Worker: $(ls -lh "$MOUNT_DIR/usr/local/bin/bastion-worker" | awk '{print $5}')"
echo "    Init script: /usr/local/bin/bastion-worker-launch.sh"
echo ""
echo "Usage with FirecrackerProvider:"
echo "  boot_args: \"console=ttyS0 reboot=k panic=1 ip=10.0.2.2::10.0.2.1:255.255.255.0::eth0:off bastion.gateway=10.0.2.1:50052 bastion.sandbox_id=<ID> bastion.secret=<SECRET> init=/usr/local/bin/bastion-worker-launch.sh\""
