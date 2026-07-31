#!/bin/bash
# run_sitl.sh — запуск ArduPilot SITL для тестирования auto-targeting
#
# Использование:
#   ./sim/sitl/run_sitl.sh start    # запустить SITL в Docker
#   ./sim/sitl/run_sitl.sh status   # проверить статус
#   ./sim/sitl/run_sitl.sh logs     # посмотреть логи
#   ./sim/sitl/run_sitl.sh stop     # остановить SITL
#   ./sim/sitl/run_sitl.sh test     # проверить MAVLink соединение
#   ./sim/sitl.sh restart   # перезапустить

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
COMPOSE_FILE="$SCRIPT_DIR/docker-compose.yml"

# Цвета для вывода
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info()  { echo -e "${BLUE}[INFO]${NC} $1"; }
log_ok()    { echo -e "${GREEN}[OK]${NC} $1"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Проверка Docker
check_docker() {
    if ! command -v docker &> /dev/null; then
        log_error "Docker не установлен. Установите: https://docs.docker.com/get-docker/"
        exit 1
    fi

    if ! docker info &> /dev/null; then
        log_error "Docker daemon не запущен. Запустите: sudo systemctl start docker"
        exit 1
    fi

    # Проверка docker compose (v2)
    if ! docker compose version &> /dev/null; then
        log_error "Docker Compose v2 не установлен. Установите: https://docs.docker.com/compose/install/"
        exit 1
    fi
}

# Запуск SITL
start_sitl() {
    log_info "Запуск ArduPilot SITL..."

    # Проверить, не запущен ли уже
    if docker compose -f "$COMPOSE_FILE" ps | grep -q "running"; then
        log_warn "SITL уже запущен. Используйте './run_sitl.sh restart' для перезапуска."
        exit 0
    fi

    # Проверить, есть ли образ (если нет — собрать)
    if ! docker image inspect auto-targeting-sitl:latest &> /dev/null; then
        log_info "Образ не найден. Первая сборка займёт ~10-15 минут..."
        log_info "Сборка ArduPilot из исходников..."
        docker compose -f "$COMPOSE_FILE" build --progress=plain
    fi

    # Запустить
    docker compose -f "$COMPOSE_FILE" up -d

    log_info "Ожидание запуска SITL..."
    log_info "Проверка MAVLink на порту 5760..."

    local retries=0
    local max_retries=60
    while [ $retries -lt $max_retries ]; do
        if nc -z 127.0.0.1 5760 2>/dev/null; then
            log_ok "SITL запущен! MAVLink доступен на tcp://127.0.0.1:5760"
            echo ""
            log_info "Подключение auto-targeting:"
            echo "  cargo run -p auto-targeting-cli -- --config $SCRIPT_DIR/sitl-config.toml -- --repl"
            echo ""
            log_info "Просмотр логов SITL:"
            echo "  ./sim/sitl/run_sitl.sh logs"
            exit 0
        fi
        retries=$((retries + 1))
        printf "\r  Ожидание... (%d/%d)" "$retries" "$max_retries"
        sleep 2
    done

    log_error "SITL не запустился за $max_retries попыток"
    log_info "Проверьте логи: ./sim/sitl/run_sitl.sh logs"
    exit 1
}

# Статус SITL
status_sitl() {
    log_info "Статус SITL:"
    docker compose -f "$COMPOSE_FILE" ps
    echo ""

    # Проверка портов
    log_info "Проверка портов:"
    for port in 5760 5762 5763; do
        if nc -z 127.0.0.1 $port 2>/dev/null; then
            log_ok "  Порт $port: открыт"
        else
            log_warn "  Порт $port: закрыт"
        fi
    done
}

# Логи SITL
logs_sitl() {
    log_info "Логи SITL (Ctrl+C для выхода):"
    docker compose -f "$COMPOSE_FILE" logs -f
}

# Остановка SITL
stop_sitl() {
    log_info "Остановка SITL..."
    docker compose -f "$COMPOSE_FILE" down
    log_ok "SITL остановлен"
}

# Перезапуск SITL
restart_sitl() {
    stop_sitl
    sleep 2
    start_sitl
}

# Тест MAVLink соединения
test_mavlink() {
    log_info "Тест MAVLink соединения с SITL..."

    if ! nc -z 127.0.0.1 5760 2>/dev/null; then
        log_error "SITL не запущен или порт 5760 недоступен"
        log_info "Запустите: ./sim/sitl/run_sitl.sh start"
        exit 1
    fi

    # Проверка с помощью Python + pymavlink (если установлен)
    if python3 -c "import pymavlink" 2>/dev/null; then
        log_info "Тест через pymavlink..."
        python3 << 'EOF'
from pymavlink import mavlink
import time

# Подключение к SITL
conn = mavlink.MAVLinkConnection("tcp:127.0.0.1:5760")

print("Ожидание heartbeat...")
for i in range(30):
    msg = conn.recv_match(type='HEARTBEAT', blocking=True, timeout=1)
    if msg:
        print(f"✅ HEARTBEAT получен! mode={msg.custom_mode}, armed={msg.base_mode & 128 != 0}")
        break
    print(f"  Ожидание... ({i+1}/30)")
else:
    print("❌ HEARTBEAT не получен за 30 секунд")
    exit(1)

print("\nТест arm/disarm...")
conn.arducopter_arm()
time.sleep(1)
msg = conn.recv_match(type='HEARTBEAT', blocking=True, timeout=2)
if msg:
    armed = msg.base_mode & 128 != 0
    print(f"  Armed: {armed}")

conn.arducopter_disarm()
time.sleep(1)
msg = conn.recv_match(type='HEARTBEAT', blocking=True, timeout=2)
if msg:
    armed = msg.base_mode & 128 != 0
    print(f"  Armed: {armed}")

print("\n✅ MAVLink тест пройден!")
conn.close()
EOF
    else
        log_warn "pymavlink не установлен. Использую простой TCP тест..."
        if echo -n "" | nc -w 2 127.0.0.1 5760; then
            log_ok "TCP соединение с 127.0.0.1:5760 установлено"
        else
            log_error "Не удалось подключиться к 127.0.0.1:5760"
            exit 1
        fi
        log_info "Для полного теста установите: pip install pymavlink"
    fi
}

# Очистка (удалить образ + логи)
clean_sitl() {
    log_warn "Это удалит Docker образ и все логи SITL!"
    read -p "Продолжить? (y/N) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        stop_sitl
        docker compose -f "$COMPOSE_FILE" down -v --rmi all
        log_ok "Очистка завершена"
    else
        log_info "Очистка отменена"
    fi
}

# Справка
help_sitl() {
    cat << EOF
Использование: $0 <command>

Команды:
  start     Запустить SITL (первая сборка ~10-15 мин)
  stop      Остановить SITL
  restart   Перезапустить SITL
  status    Показать статус контейнера и портов
  logs      Показать логи SITL (Ctrl+C для выхода)
  test      Проверить MAVLink соединение
  clean     Удалить образ и логи (полная очистка)
  help      Показать эту справку

Примеры:
  $0 start
  $0 test
  $0 logs
EOF
}

# Главная логика
case "${1:-help}" in
    start)   start_sitl ;;
    stop)    stop_sitl ;;
    restart) restart_sitl ;;
    status)  status_sitl ;;
    logs)    logs_sitl ;;
    test)    test_mavlink ;;
    clean)   clean_sitl ;;
    help|--help|-h) help_sitl ;;
    *)
        log_error "Неизвестная команда: $1"
        help_sitl
        exit 1
        ;;
esac
