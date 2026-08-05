#!/bin/bash
# setup_orange_pi.sh — автонастройка Orange Pi 5 для auto-targeting
#
# Запускать НА Orange Pi 5 (через SSH):
#   ssh orangepi@192.168.1.XXX
#   curl -sSL https://raw.githubusercontent.com/EgorLikhachev/Autotargeting/main/scripts/setup_orange_pi.sh | bash -
#
# Или скачать и запустить:
#   wget https://raw.githubusercontent.com/EgorLikhachev/Autotargeting/main/scripts/setup_orange_pi.sh
#   chmod +x setup_orange_pi.sh
#   ./setup_orange_pi.sh

set -euo pipefail

# Цвета
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()  { echo -e "${BLUE}[INFO]${NC} $1"; }
log_ok()    { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

echo "=== Auto-Targeting System — Orange Pi 5 Setup ==="
echo ""

# Проверка что мы на Orange Pi
if ! grep -qi "orange" /proc/device-tree/compatible 2>/dev/null; then
    log_warn "This doesn't appear to be an Orange Pi. Continue anyway? (y/N)"
    read -r response
    [[ "$response" =~ ^[Yy]$ ]] || exit 0
fi

# Проверка ОС
if [[ -f /etc/armbian-release ]]; then
    log_ok "OS: Armbian detected"
elif [[ -f /etc/lsb-release ]] && grep -qi ubuntu /etc/lsb-release; then
    log_ok "OS: Ubuntu detected"
else
    log_warn "OS: unknown (proceeding anyway)"
fi

# 1. Обновление системы
log_info "Updating system packages..."
sudo apt update -qq
sudo apt upgrade -y -qq

# 2. Установка зависимостей
log_info "Installing dependencies..."
sudo apt install -y -qq \
    build-essential \
    cmake \
    git \
    curl \
    wget \
    v4l-utils \
    usbutils \
    ffmpeg \
    htop \
    net-tools \
    libclang-dev \
    pkg-config

log_ok "Dependencies installed"

# 3. Установка Rust
if ! command -v cargo &> /dev/null; then
    log_info "Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
    source ~/.cargo/env
    log_ok "Rust installed: $(rustc --version)"
else
    log_ok "Rust already installed: $(rustc --version)"
fi

# 4. Настройка пользователя
CURRENT_USER=$(whoami)
log_info "Adding user '$CURRENT_USER' to video and dialout groups..."
sudo usermod -aG video,dialout "$CURRENT_USER"
log_ok "User groups updated (relogin to apply)"

# 5. Swap (если мало памяти)
TOTAL_MEM=$(free -m | awk '/^Mem:/{print $2}')
if [[ $TOTAL_MEM -lt 4000 ]]; then
    log_info "Memory: ${TOTAL_MEM}MB — creating 4GB swap..."
    if [[ ! -f /swapfile ]]; then
        sudo fallocate -l 4G /swapfile
        sudo chmod 600 /swapfile
        sudo mkswap /swapfile
        sudo swapon /swapfile
        echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab > /dev/null
        log_ok "Swap created (4GB)"
    else
        log_ok "Swap already exists"
    fi
else
    log_ok "Memory: ${TOTAL_MEM}MB — sufficient, no swap needed"
fi

# 6. Клонирование проекта
if [[ ! -d ~/auto-targeting ]]; then
    log_info "Cloning auto-targeting repository..."
    cd ~
    git clone https://github.com/EgorLikhachev/Autotargeting.git auto-targeting
    log_ok "Repository cloned"
else
    log_ok "Repository already exists at ~/auto-targeting"
    log_info "Updating..."
    cd ~/auto-targeting
    git pull --rebase
fi

# 7. Сборка
cd ~/auto-targeting
log_info "Building auto-targeting with V4L2 support..."
log_info "This will take ~10-15 minutes on Orange Pi 5..."

if cargo build --release --features video-capture/v4l2 -p auto-targeting-cli 2>&1 | tail -5; then
    log_ok "Build successful!"
    log_info "Binary: $(ls -la target/release/auto-targeting)"
else
    log_error "Build failed!"
    log_info "Try: cargo build --release -p auto-targeting-cli (without v4l2 feature)"
    exit 1
fi

# 8. Проверка камеры
echo ""
log_info "=== Camera Check ==="
if ls /dev/video* 2>/dev/null; then
    log_ok "V4L2 device(s) found:"
    ls -la /dev/video*

    echo ""
    log_info "USB devices:"
    lsusb

    echo ""
    log_info "Camera formats:"
    for dev in /dev/video*; do
        echo "  $dev:"
        v4l2-ctl --device "$dev" --list-formats-ext 2>/dev/null | head -20
    done
else
    log_warn "No V4L2 devices found!"
    log_info "Connect USB camera and run: lsusb"
fi

# 9. Smoke test
echo ""
log_info "=== Smoke Test ==="
./target/release/auto-targeting --mock-all 2>&1 | tail -20

echo ""
log_info "=== Setup Complete ==="
echo ""
echo "Next steps:"
echo "  1. Relogin to apply group changes: exit && ssh $CURRENT_USER@$(hostname -I)"
echo "  2. Run REPL: ~/auto-targeting/target/release/auto-targeting --repl"
echo "  3. Run scenarios: ~/auto-targeting/target/release/auto-targeting scenario --all ~/auto-targeting/sim/scenarios/"
echo "  4. Run hardware tests: ~/auto-targeting/scripts/run_hardware_tests.sh"
echo ""
echo "Documentation:"
echo "  - Hardware setup: ~/auto-targeting/docs/HARDWARE_SETUP.md"
echo "  - Manual testing: ~/auto-targeting/docs/MANUAL_TESTING.md"
echo ""
