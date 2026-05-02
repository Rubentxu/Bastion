#!/bin/sh
# Bastion Worker Bootstrap for Lambda/FaaS
#
# Usage: ./bootstrap-worker.sh
# 
# Environment:
#   BASTION_WORKER_URL   - URL to download worker binary
#   BASTION_WORKER_SHA256 - Expected sha256 (optional)
#   BASTION_GATEWAY_ADDR - Gateway address
#   BASTION_SANDBOX_ID   - Sandbox ID  
#   BASTION_AUTH_TOKEN   - Auth token

set -e

WORKER_URL="${BASTION_WORKER_URL}"
EXPECTED_SHA256="${BASTION_WORKER_SHA256:-}"
WORKER_PATH="/tmp/bastion-worker"

if [ -z "$WORKER_URL" ]; then
    echo "ERROR: BASTION_WORKER_URL is required"
    exit 1
fi

# Download worker
echo "Downloading worker from $WORKER_URL"
if command -v curl >/dev/null 2>&1; then
    curl -fL "$WORKER_URL" -o "$WORKER_PATH"
elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$WORKER_PATH" "$WORKER_URL"
else
    echo "ERROR: curl or wget is required"
    exit 1
fi

# Verify sha256 if provided
if [ -n "$EXPECTED_SHA256" ]; then
    ACTUAL_SHA256=$(sha256sum "$WORKER_PATH" | cut -d' ' -f1)
    if [ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]; then
        echo "ERROR: SHA256 mismatch"
        rm -f "$WORKER_PATH"
        exit 1
    fi
    echo "SHA256 verified"
fi

# Make executable
chmod +x "$WORKER_PATH"

# Execute worker
exec "$WORKER_PATH" \
    --gateway-addr "${BASTION_GATEWAY_ADDR:-http://localhost:50052}" \
    --sandbox-id "${BASTION_SANDBOX_ID:-unknown}" \
    --secret "${BASTION_AUTH_TOKEN:-}" \
    --workdir /workspace
