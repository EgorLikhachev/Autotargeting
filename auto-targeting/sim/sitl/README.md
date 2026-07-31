# SITL — Software In The Loop

This directory contains the SITL test infrastructure.

## Status

🚧 **Phase 0 stub.** The `docker-compose.yml` is the target shape; actual
ArduPilot SITL integration lands in Phase 6.

## Usage (once Phase 6 lands)

```bash
# Start SITL
docker compose up -d

# Verify it's running
docker compose ps

# MAVLink is now available at:
#   - tcp://127.0.0.1:5760  (QGroundControl / MAVProxy)
#   - udp://127.0.0.1:5763  (companion computer — our auto-targeting)

# Run auto-targeting against SITL
cargo run -p auto-targeting-cli -- --config ../configs/sitl.toml

# Stop SITL
docker compose down
```

## Local dev without Docker

If you have ArduPilot built locally:

```bash
# Start SITL with default plane model
sim_vehicle.py -v ArduPlane -f plane --map --console

# In another terminal, MAVProxy connects to tcp:127.0.0.1:5760
# Our auto-targeting connects to udp:127.0.0.1:14550 (default MAVProxy output)
```

## Test scenarios

Test scenarios live in `../scenarios/`. Each scenario is a JSON file with:
- A replay video file (or synthetic generator config)
- Expected events (state transitions, watchdog triggers, etc.)

Phase 6 will populate this directory with the standard scenario suite.
