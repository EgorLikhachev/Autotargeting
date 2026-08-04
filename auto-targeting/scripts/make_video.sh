#!/usr/bin/env bash
# Mux the annotated JPEG frames saved by cv-visualizer into an MP4 video.
#
# This closes the Phase 1.1 criterion "сохранён пример обработанного видео":
# the on-board runtime stays headless (no video encoder in the binary), and
# this script runs offline (on the dev machine or after copying frames off
# the board) to produce the demo video.
#
# Usage:
#   ./scripts/make_video.sh [OUTPUT_DIR] [FPS]
#
# Defaults:
#   OUTPUT_DIR = ./output
#   FPS        = 15
#
# Requirements:
#   - ffmpeg in PATH (apt-get install ffmpeg on Debian/Ubuntu).
#   - Annotated JPEGs at OUTPUT_DIR/frames/seq_NNNNNN.jpg (produced by a
#     cv-visualizer FrameWriter run, e.g. the soak test).
set -euo pipefail

OUTPUT_DIR="${1:-output}"
FPS="${2:-15}"
FRAMES_DIR="${OUTPUT_DIR}/frames"
OUT_VIDEO="${OUTPUT_DIR}/processed.mp4"

if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "[make_video] ERROR: ffmpeg not found in PATH." >&2
    echo "[make_video] Install:  sudo apt-get install ffmpeg" >&2
    exit 1
fi

if [[ ! -d "${FRAMES_DIR}" ]]; then
    echo "[make_video] ERROR: frames dir not found: ${FRAMES_DIR}" >&2
    echo "[make_video] Run a cv-visualizer session first (e.g. the soak test)." >&2
    exit 1
fi

N=$(find "${FRAMES_DIR}" -name 'seq_*.jpg' | wc -l)
if [[ "${N}" -eq 0 ]]; then
    echo "[make_video] ERROR: no seq_*.jpg frames in ${FRAMES_DIR}" >&2
    exit 1
fi

echo "[make_video] ${N} frames in ${FRAMES_DIR}, fps=${FPS}"
echo "[make_video] -> ${OUT_VIDEO}"

# -framerate BEFORE -i so ffmpeg interprets the image sequence at that rate.
# -pattern_type sequence + seq_%06d.jpg matches FrameWriter's naming.
# -pix_fmt yuv420p for broad player compatibility.
# -crf 23 is a sane default (lower = higher quality / larger file).
ffmpeg -y \
    -framerate "${FPS}" \
    -i "${FRAMES_DIR}/seq_%06d.jpg" \
    -c:v libx264 \
    -pix_fmt yuv420p \
    -crf 23 \
    -preset medium \
    "${OUT_VIDEO}"

echo "[make_video] OK: ${OUT_VIDEO}"
