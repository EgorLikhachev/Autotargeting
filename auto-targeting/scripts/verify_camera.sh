#!/bin/bash
# verify_camera.sh — проверка USB камеры на Orange Pi 5
#
# Запускать НА Orange Pi 5:
#   ./scripts/verify_camera.sh
#
# Или:
#   ./scripts/verify_camera.sh /dev/video0

set -euo pipefail

DEVICE="${1:-/dev/video0}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()  { echo -e "${BLUE}[INFO]${NC} $1"; }
log_ok()    { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

echo "=== Camera Verification ==="
echo "Device: $DEVICE"
echo ""

# 1. Проверка устройства
log_info "Checking device $DEVICE..."
if [[ ! -e "$DEVICE" ]]; then
    log_error "Device $DEVICE does not exist!"
    log_info "Available V4L2 devices:"
    ls /dev/video* 2>/dev/null || echo "  No /dev/video* devices found"
    log_info "USB devices:"
    lsusb
    exit 1
fi
log_ok "Device exists"

# 2. Права доступа
log_info "Checking permissions..."
if [[ -r "$DEVICE" && -w "$DEVICE" ]]; then
    log_ok "Device is readable/writable"
else
    log_warn "Device is not accessible"
    log_info "Current user: $(whoami)"
    log_info "Groups: $(groups)"
    log_info "Fix: sudo usermod -aG video $(whoami) && relogin"
    exit 1
fi

# 3. USB информация
echo ""
log_info "=== USB Information ==="
lsusb
echo ""

# 4. V4L2 форматы
log_info "=== V4L2 Formats for $DEVICE ==="
if ! v4l2-ctl --device "$DEVICE" --list-formats-ext 2>/dev/null; then
    log_error "v4l2-ctl failed to query formats"
    log_info "Install: sudo apt install v4l-utils"
    exit 1
fi

# 5. Поддерживаемые форматы
echo ""
log_info "=== Format Analysis ==="
FORMATS=$(v4l2-ctl --device "$DEVICE" --list-formats-ext 2>/dev/null)

if echo "$FORMATS" | grep -qi "MJPG"; then
    log_ok "MJPEG supported (preferred for USB cameras)"
    PREFERRED_FORMAT="mjpeg"
elif echo "$FORMATS" | grep -qi "YUYV"; then
    log_ok "YUYV supported"
    PREFERRED_FORMAT="yuyv"
elif echo "$FORMATS" | grep -qi "RGB"; then
    log_ok "RGB24 supported"
    PREFERRED_FORMAT="rgb24"
else
    log_error "No supported formats found (need MJPG, YUYV, or RGB)"
    exit 1
fi

# 6. Разрешения
log_info "Checking resolutions..."
if echo "$FORMATS" | grep -q "1280x720"; then
    log_ok "720p (1280x720) supported"
    PREFERRED_RES="1280x720"
elif echo "$FORMATS" | grep -q "640x480"; then
    log_ok "VGA (640x480) supported"
    PREFERRED_RES="640x480"
else
    log_warn "No standard resolutions found"
    PREFERRED_RES=""
fi

# 7. Тестовая запись
echo ""
log_info "=== Test Recording (5 seconds) ==="
TEST_FILE="/tmp/camera_test_$(date +%s).mp4"

if [[ -n "$PREFERRED_RES" ]]; then
    log_info "Recording $PREFERRED_RES @ $PREFERRED_FORMAT for 5 seconds..."

    if ffmpeg -y -f v4l2 \
        -input_format "$PREFERRED_FORMAT" \
        -video_size "$PREFERRED_RES" \
        -i "$DEVICE" \
        -t 5 \
        "$TEST_FILE" 2>&1 | tail -10; then

        if [[ -f "$TEST_FILE" ]]; then
            FILE_SIZE=$(stat -c%s "$TEST_FILE")
            log_ok "Recording successful: $TEST_FILE ($FILE_SIZE bytes)"

            if [[ $FILE_SIZE -gt 1000 ]]; then
                log_ok "File size > 1KB — video has content"
            else
                log_warn "File is very small — video might be empty"
            fi

            echo ""
            log_info "To view on your computer:"
            echo "  scp $(whoami)@$(hostname -I | awk '{print $1}'):$TEST_FILE ."
            echo "  ffplay $(basename $TEST_FILE)"
        else
            log_error "Recording failed — no file created"
            exit 1
        fi
    else
        log_error "ffmpeg recording failed"
        log_info "Try: ffmpeg -f v4l2 -list_formats all -i $DEVICE"
        exit 1
    fi
else
    log_warn "Skipping recording (no standard resolution)"
fi

# 8. Latency test
echo ""
log_info "=== Latency Test (10 frames) ==="
log_info "Measuring capture latency..."

START_TIME=$(date +%s%N)
ffmpeg -y -f v4l2 \
    -input_format "$PREFERRED_FORMAT" \
    -video_size "${PREFERRED_RES:-640x480}" \
    -i "$DEVICE" \
    -frames:v 10 \
    /tmp/latency_test_%03d.jpg 2>/dev/null || true
END_TIME=$(date +%s%N)

ELAPSED_MS=$(( (END_TIME - START_TIME) / 1000000 ))
PER_FRAME_MS=$(( ELAPSED_MS / 10 ))

log_info "Captured 10 frames in ${ELAPSED_MS}ms (${PER_FRAME_MS}ms/frame)"

if [[ $PER_FRAME_MS -lt 50 ]]; then
    log_ok "Latency < 50ms per frame — excellent"
elif [[ $PER_FRAME_MS -lt 100 ]]; then
    log_ok "Latency < 100ms per frame — acceptable"
else
    log_warn "Latency > 100ms per frame — may need optimization"
fi

# Cleanup
rm -f /tmp/latency_test_*.jpg

# 9. auto-targeting probe (если собран)
echo ""
log_info "=== auto-targeting V4l2Source Probe ==="

# Найти бинарник
BINARY=""
for path in \
    ~/auto-targeting/target/release/auto-targeting \
    ./target/release/auto-targeting \
    ../target/release/auto-targeting; do
    if [[ -x "$path" ]]; then
        BINARY="$path"
        break
    fi
done

if [[ -n "$BINARY" ]]; then
    log_info "Found binary: $BINARY"

    # Создать временный Rust файл для probe
    PROBE_SRC=$(mktemp /tmp/v4l2_probe_XXXXXX.rs)
    cat > "$PROBE_SRC" << 'EOF'
// Probe V4L2 device capabilities
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let device = args.get(1).cloned().unwrap_or_else(|| "/dev/video0".to_string());
    println!("Probing: {}", device);
    // Real probe would use V4l2Source::probe()
    println!("(Run auto-targeting with v4l2 feature to test)")
}
EOF
    log_info "Binary check: $BINARY --health-check"
    if "$BINARY" --health-check 2>/dev/null; then
        log_ok "auto-targeting binary works"
    else
        log_warn "Binary health check failed (may need different config)"
    fi
else
    log_warn "auto-targeting binary not found"
    log_info "Build: cd ~/auto-targeting && cargo build --release --features video-capture/v4l2"
fi

# 10. Итог
echo ""
log_info "=== Summary ==="
echo "  Device: $DEVICE"
echo "  Preferred format: $PREFERRED_FORMAT"
echo "  Preferred resolution: ${PREFERRED_RES:-unknown}"
echo "  Latency per frame: ${PER_FRAME_MS}ms"
echo "  Test video: $TEST_FILE"
echo ""
log_ok "Camera verification complete!"
echo ""
echo "Next: configure auto-targeting to use this camera"
echo "  cp config.example.toml config.camera.toml"
echo "  nano config.camera.toml  # set device, format, resolution"
echo "  ./auto-targeting --config config.camera.toml --repl"
