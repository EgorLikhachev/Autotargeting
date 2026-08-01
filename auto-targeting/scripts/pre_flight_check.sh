#!/bin/bash
# pre_flight_check.sh — предполётная проверка системы
#
# Запускается ПЕРЕД каждым полётом (или HITL тестом).
# Проверяет, что все подсистемы готовы.
#
# Использование:
#   ./scripts/pre_flight_check.sh
#   ./scripts/pre_flight_check.sh --strict   # fail on any warning

set -uo pipefail

STRICT="${1:-}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

PASS=0
FAIL=0
WARN=0

log_info()  { echo -e "${BLUE}[INFO]${NC} $1"; }
log_ok()    { echo -e "${GREEN}[✓]${NC} $1"; PASS=$((PASS + 1)); }
log_fail()  { echo -e "${RED}[✗]${NC} $1"; FAIL=$((FAIL + 1)); }
log_warn()  { echo -e "${YELLOW}[!]${NC} $1"; WARN=$((WARN + 1)); }
log_section() { echo -e "\n${BLUE}=== $1 ===${NC}"; }

echo "╔══════════════════════════════════════╗"
echo "║   PRE-FLIGHT CHECKLIST               ║"
echo "╚══════════════════════════════════════╝"
echo "Date: $(date)"
echo ""

# === 1. Software checks ===
log_section "1. Software"

# Binary exists
BINARY=""
for path in \
    ./target/release/auto-targeting \
    ~/auto-targeting/target/release/auto-targeting \
    /opt/auto-targeting/bin/auto-targeting; do
    if [[ -x "$path" ]]; then
        BINARY="$path"
        break
    fi
done

if [[ -n "$BINARY" ]]; then
    log_ok "Binary found: $BINARY"
else
    log_fail "Binary not found"
    log_info "Build: cargo build --release --features video-capture/v4l2"
fi

# Binary works
if [[ -n "$BINARY" ]] && "$BINARY" --health-check 2>/dev/null | grep -q '"status":"ok"'; then
    log_ok "Binary health check OK"
else
    log_fail "Binary health check failed"
fi

# Config file
CONFIG="${CONFIG:-config.toml}"
if [[ -f "$CONFIG" ]]; then
    log_ok "Config file: $CONFIG"
else
    log_warn "Config file not found: $CONFIG"
    log_info "Using defaults"
fi

# === 2. Camera checks ===
log_section "2. Camera"

if ls /dev/video* 2>/dev/null; then
    CAMERA_DEV=$(ls /dev/video* | head -1)
    log_ok "Camera device: $CAMERA_DEV"

    # Check permissions
    if [[ -r "$CAMERA_DEV" ]]; then
        log_ok "Camera readable"
    else
        log_fail "Camera not readable (check groups: sudo usermod -aG video $USER)"
    fi

    # Check formats
    FORMATS=$(v4l2-ctl --device "$CAMERA_DEV" --list-formats-ext 2>/dev/null)
    if echo "$FORMATS" | grep -qi "MJPG\|YUYV"; then
        log_ok "Camera supports MJPEG/YUYV"
    else
        log_fail "Camera doesn't support MJPEG or YUYV"
    fi

    # Quick capture test (1 frame)
    if ffmpeg -y -f v4l2 -input_format mjpeg -video_size 1280x720 \
        -i "$CAMERA_DEV" -frames:v 1 /tmp/preflight_test.jpg 2>/dev/null; then
        FILE_SIZE=$(stat -c%s /tmp/preflight_test.jpg 2>/dev/null || echo 0)
        if [[ $FILE_SIZE -gt 1000 ]]; then
            log_ok "Camera capture test OK (${FILE_SIZE} bytes)"
        else
            log_fail "Camera capture produced empty frame"
        fi
        rm -f /tmp/preflight_test.jpg
    else
        log_fail "Camera capture test failed"
    fi
else
    log_warn "No camera detected"
    if [[ "$STRICT" == "--strict" ]]; then
        log_fail "Camera required in strict mode"
    fi
fi

# === 3. FC checks (if connected) ===
log_section "3. Flight Controller"

FC_DEVICE=""
for dev in /dev/ttyACM0 /dev/ttyUSB0 /dev/ttyACM1 /dev/ttyUSB1; do
    if [[ -e "$dev" ]]; then
        FC_DEVICE="$dev"
        break
    fi
done

if [[ -n "$FC_DEVICE" ]]; then
    log_ok "FC device: $FC_DEVICE"

    # Check permissions
    if [[ -r "$FC_DEVICE" ]] && [[ -w "$FC_DEVICE" ]]; then
        log_ok "FC device accessible"
    else
        log_fail "FC not accessible (check groups: sudo usermod -aG dialout $USER)"
    fi

    # Check if device responds (simple read test)
    if timeout 2 cat "$FC_DEVICE" > /dev/null 2>&1; then
        log_ok "FC device responds"
    else
        log_warn "FC device read timeout (may need specific baud rate)"
    fi
else
    log_warn "No FC detected (will use mock adapter)"
fi

# === 4. Network checks ===
log_section "4. Network"

# Check internet (for NTP)
if ping -c 1 -W 2 8.8.8.8 > /dev/null 2>&1; then
    log_ok "Internet connection OK"
else
    log_warn "No internet (NTP sync may fail)"
fi

# Check local network
LOCAL_IP=$(hostname -I | awk '{print $1}')
if [[ -n "$LOCAL_IP" ]]; then
    log_ok "Local IP: $LOCAL_IP"
else
    log_fail "No network interface"
fi

# === 5. Disk space ===
log_section "5. Disk Space"

DISK_FREE=$(df / | awk 'NR==2 {print $4}')
DISK_FREE_GB=$((DISK_FREE / 1024 / 1024))

if [[ $DISK_FREE_GB -gt 1 ]]; then
    log_ok "Disk space: ${DISK_FREE_GB}GB free"
else
    log_fail "Disk space low: ${DISK_FREE_GB}GB free (need >1GB)"
fi

# Logs directory
LOG_DIR="/var/log/auto-targeting"
if [[ -d "$LOG_DIR" ]]; then
    log_ok "Log directory exists: $LOG_DIR"
else
    log_warn "Log directory missing: $LOG_DIR"
    log_info "Create: sudo mkdir -p $LOG_DIR && sudo chown $USER $LOG_DIR"
fi

# === 6. Memory ===
log_section "6. Memory"

TOTAL_MEM=$(free -m | awk '/^Mem:/{print $2}')
FREE_MEM=$(free -m | awk '/^Mem:/{print $4}')

log_info "Total memory: ${TOTAL_MEM}MB"
log_info "Free memory: ${FREE_MEM}MB"

if [[ $FREE_MEM -gt 500 ]]; then
    log_ok "Sufficient free memory (${FREE_MEM}MB)"
else
    log_warn "Low memory (${FREE_MEM}MB free)"
    if [[ "$STRICT" == "--strict" ]]; then
        log_fail "Memory too low in strict mode"
    fi
fi

# Swap
SWAP_TOTAL=$(free -m | awk '/^Swap:/{print $2}')
if [[ $SWAP_TOTAL -gt 0 ]]; then
    log_ok "Swap enabled: ${SWAP_TOTAL}MB"
else
    log_warn "No swap — may cause OOM during build"
fi

# === 7. System load ===
log_section "7. System Load"

LOAD_1MIN=$(awk '{print $1}' /proc/loadavg)
LOAD_THRESHOLD="2.0"

# Compare using awk (float comparison)
if awk "BEGIN {exit !($LOAD_1MIN < $LOAD_THRESHOLD)}"; then
    log_ok "System load: $LOAD_1MIN (OK)"
else
    log_warn "System load high: $LOAD_1MIN"
fi

# CPU temperature (if available)
if [[ -f /sys/class/thermal/thermal_zone0/temp ]]; then
    TEMP_RAW=$(cat /sys/class/thermal/thermal_zone0/temp)
    TEMP_C=$((TEMP_RAW / 1000))
    if [[ $TEMP_C -lt 70 ]]; then
        log_ok "CPU temperature: ${TEMP_C}°C"
    elif [[ $TEMP_C -lt 85 ]]; then
        log_warn "CPU temperature: ${TEMP_C}°C (warm)"
    else
        log_fail "CPU temperature: ${TEMP_C}°C (too hot!)"
    fi
fi

# === 8. Services (if installed) ===
log_section "8. Systemd Services"

if systemctl is-active --quiet auto-targeting.service 2>/dev/null; then
    log_ok "auto-targeting.service: active"
elif systemctl exists auto-targeting.service 2>/dev/null; then
    log_warn "auto-targeting.service: inactive"
else
    log_info "auto-targeting.service: not installed"
fi

if systemctl is-active --quiet rknn-bridge.service 2>/dev/null; then
    log_ok "rknn-bridge.service: active"
elif systemctl exists rknn-bridge.service 2>/dev/null; then
    log_warn "rknn-bridge.service: inactive"
else
    log_info "rknn-bridge.service: not installed"
fi

# === Summary ===
echo ""
echo "╔══════════════════════════════════════╗"
echo "║   PRE-FLIGHT CHECK SUMMARY           ║"
echo "╠══════════════════════════════════════╣"
echo -e "║  ${GREEN}PASS${NC}: $PASS                              ║"
echo -e "║  ${RED}FAIL${NC}: $FAIL                              ║"
echo -e "║  ${YELLOW}WARN${NC}: $WARN                              ║"
echo "╚══════════════════════════════════════╝"

if [[ $FAIL -gt 0 ]]; then
    echo -e "${RED}Cannot proceed — $FAIL critical issues${NC}"
    exit 1
elif [[ $WARN -gt 0 ]] && [[ "$STRICT" == "--strict" ]]; then
    echo -e "${YELLOW}$WARN warnings in strict mode${NC}"
    exit 2
else
    echo -e "${GREEN}All critical checks passed${NC}"
    if [[ $WARN -gt 0 ]]; then
        echo -e "${YELLOW}($WARN warnings — review above)${NC}"
    fi
    echo ""
    echo "Ready for: auto-targeting --config config.toml --repl"
    exit 0
fi
