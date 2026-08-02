# Dockerfile для auto-targeting (companion computer)
#
# Собирает auto-targeting для aarch64 (Orange Pi 5) в многостадийной сборке.
# Результирующий образ минимален (~50MB).
#
# Build:
#   docker build -t auto-targeting:latest -f deploy/Dockerfile .
#
# Run (with mock FC, for testing):
#   docker run --rm -p 8080:8080 auto-targeting:latest --mock-all
#
# Run (with real FC, requires device access):
#   docker run --rm --device /dev/video0 --device /dev/ttyACM0 \
#     -p 8080:8080 -v /etc/auto-targeting:/etc/auto-targeting:ro \
#     auto-targeting:latest --config /etc/auto-targeting/config.toml

# === Stage 1: Builder ===
FROM rust:1.75-bookworm AS builder

WORKDIR /build

# Install dependencies for v4l2 and cross-compilation
RUN apt-get update && apt-get install -y \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy workspace
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY config.example.toml ./

# Build release binary
RUN cargo build --release --features video-capture/v4l2 -p auto-targeting-cli

# === Stage 2: Runtime ===
FROM debian:bookworm-slim

LABEL maintainer="Auto-Targeting Team"
LABEL description="Auto-Targeting companion computer"

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    libclang-14-runtime \
    v4l-utils \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -r -s /bin/false -G video,dialout autotarget

# Create directories
WORKDIR /opt/auto-targeting
RUN mkdir -p bin models /var/log/auto-targeting /etc/auto-targeting && \
    chown -R autotarget:autotarget /opt/auto-targeting /var/log/auto-targeting

# Copy binary from builder
COPY --from=builder /build/target/release/auto-targeting /opt/auto-targeting/bin/auto-targeting

# Copy config template
COPY config.example.toml /etc/auto-targeting/config.toml

# Set permissions
RUN chmod +x /opt/auto-targeting/bin/auto-targeting && \
    chown -R autotarget:autotarget /etc/auto-targeting

# Switch to non-root user
USER autotarget

# Expose health endpoint
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/opt/auto-targeting/bin/auto-targeting", "--health-check"] || exit 1

# Default command (override with --config for production)
ENTRYPOINT ["/opt/auto-targeting/bin/auto-targeting"]
CMD ["--config", "/etc/auto-targeting/config.toml"]
