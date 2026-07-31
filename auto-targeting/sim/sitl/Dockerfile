# Dockerfile для ArduPilot SITL
#
# Собирает ArduPilot из исходников для SITL (Software In The Loop) тестирования.
# Первая сборка — ~10-15 минут. Последующие — мгновенно (кэш).
#
# Образ основан на Ubuntu 22.04 (как Orange Pi 5).

FROM ubuntu:22.04

LABEL maintainer="Auto-Targeting Team"
LABEL description="ArduPilot SITL for auto-targeting testing"

# Избегаем интерактивных prompt'ов при установке
ENV DEBIAN_FRONTEND=noninteractive
ENV ARDUPILOT_REPO=https://github.com/ArduPilot/ardupilot
ENV ARDUPILOT_BRANCH=Plane-4.5

# Устанавливаем зависимости ArduPilot
RUN apt-get update && apt-get install -y \
    build-essential \
    cmake \
    git \
    python3 \
    python3-pip \
    python3-dev \
    python3-setuptools \
    wget \
    curl \
    libtool \
    libxml2-dev \
    libxslt1-dev \
    python3-lxml \
    libboost-all-dev \
    lib-trng-dev \
    gcc-arm-none-eabi \
    binutils-arm-none-eabi \
    software-properties-common \
    xterm \
    screen \
    tmux \
    && rm -rf /var/lib/apt/lists/*

# Устанавливаем MAVProxy (для --map и --console, опционально)
RUN pip3 install --no-cache-dir \
    pymavlink \
    mavproxy \
    matplotlib \
    numpy \
    future \
    empy==3.3.4 \
    ptyprocess

# Клонируем ArduPilot
WORKDIR /ardupilot
RUN git clone --depth 1 --branch ${ARDUPILOT_BRANCH} ${ARDUPILOT_REPO} /ardupilot && \
    git submodule update --init --recursive --depth 1

# Собираем SITL для Plane
# Это занимает ~10 минут при первой сборке
RUN ./waf configure --board sitl && \
    ./waf plane

# Создаём рабочую директорию для запуска
WORKDIR /sitl_work
RUN mkdir -p /sitl_work/logs

# Порт MAVLink TCP (по умолчанию 5760)
EXPOSE 5760 5762 5763 14550

# Точка входа — sim_vehicle.py
# Параметры передаются через docker-compose.yml
ENTRYPOINT ["python3", "/ardupilot/Tools/autotest/sim_vehicle.py"]
CMD ["--vehicle", "ArduPlane", "--frame", "plane", "--no-mavproxy", "--instance", "1"]
