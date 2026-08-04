#!/usr/bin/env bash
# Run the Phase 1.1 30-minute soak test of the minimal CV loop.
#
# Closes the criterion "непрерывный тест продолжительностью не менее 30 минут"
# of task 1.1. Runs the `soak` example for 30 minutes on a synthetic video
# source (no camera needed) with the COCO YOLOv8n model (if available), and
# produces:
#
#   output/soak/
#     frames/seq_NNNNNN.jpg   annotated frames (~1 Hz, throttled)
#     detections.jsonl        per-saved-frame detection log
#     telemetry.jsonl         periodic RSS/temperature samples
#     summary.json            final FPS + latency percentiles
#
# Plus a processed-video demo at output/soak/processed.mp4 (via make_video.sh).
#
# Usage:
#   ./scripts/soak_30min.sh [MINUTES] [MODEL_PATH]
#
# Defaults:
#   MINUTES    = 30 (the 1.1 minimum)
#   MODEL_PATH = models/yolov8n.onnx (downloaded by download_models.sh)
#
# Requirements:
#   - Built `soak` example (this script builds it with --features cpu-onnx).
#   - ffmpeg in PATH (optional; for the processed-video mux at the end).
set -euo pipefail

MINUTES="${1:-30}"
MODEL_PATH="${2:-models/yolov8n.onnx}"
OUTPUT_DIR="output/soak"
FONT_PATH="${FONT_PATH:-/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf}"

echo "[soak] duration:   ${MINUTES} min"
echo "[soak] model:      ${MODEL_PATH}"
echo "[soak] output dir: ${OUTPUT_DIR}"

# Build the example once (release for representative perf numbers).
echo "[soak] building soak example (release, cpu-onnx)..."
cargo build --release -p cv-inference --example soak --features cpu-onnx

MODEL_ARG=""
if [[ -f "${MODEL_PATH}" ]]; then
    MODEL_ARG="--model ${MODEL_PATH}"
    echo "[soak] model present — full inference soak"
else
    echo "[soak] WARNING: model not found at ${MODEL_PATH}"
    echo "[soak]   run ./scripts/download_models.sh first, or continue in smoke mode (no inference)"
fi

FONT_ARG=""
if [[ -f "${FONT_PATH}" ]]; then
    FONT_ARG="--font ${FONT_PATH}"
fi

# Run the soak. The example writes all artifacts under OUTPUT_DIR.
"$(cargo target-dir --release 2>/dev/null || echo target/release)/examples/soak" \
    --minutes "${MINUTES}" \
    --output "${OUTPUT_DIR}" \
    ${MODEL_ARG} \
    ${FONT_ARG}

echo ""
echo "[soak] artifacts:"
echo "  ${OUTPUT_DIR}/summary.json"
echo "  ${OUTPUT_DIR}/telemetry.jsonl"
echo "  ${OUTPUT_DIR}/detections.jsonl"

# Optional: mux annotated frames into a demo video.
if command -v ffmpeg >/dev/null 2>&1; then
    echo ""
    ./scripts/make_video.sh "${OUTPUT_DIR}" 15 || echo "[soak] video mux skipped (no frames?)"
else
    echo "[soak] ffmpeg not present; skipping processed-video mux."
fi
