#!/bin/bash
# Health check script — used by Ansible deploy to verify the service is up.
# Returns 0 if healthy, 1 otherwise.

set -euo pipefail

CONFIG_PATH="${1:-/etc/auto-targeting/config.toml}"
HEALTH_TIMEOUT=5  # seconds

# Try to invoke the binary's health check
if /opt/auto-targeting/bin/auto-targeting --health-check > /tmp/health-output.json 2>&1; then
    # Verify the JSON output contains "ok"
    if grep -q '"status":"ok"' /tmp/health-output.json; then
        echo "OK: auto-targeting is healthy"
        cat /tmp/health-output.json
        exit 0
    fi
fi

echo "FAIL: auto-targeting health check failed"
cat /tmp/health-output.json 2>/dev/null || true
exit 1
