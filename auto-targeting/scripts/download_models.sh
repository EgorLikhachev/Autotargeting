#!/usr/bin/env bash
# Download the baseline COCO YOLOv8n ONNX model for Phase 1.1.
#
# The model is NOT stored in git (large binary). This script fetches a known
# Ultralytics YOLOv8n export that matches the layout our `yolov8` parser
# expects: input  [1,3,640,640] float32, output [1,84,8400] float32.
#
# Usage:
#   ./scripts/download_models.sh
#
# Output:
#   models/yolov8n.onnx
#
# Notes:
# - Requires curl and unzip. On Orange Pi / Ubuntu: sudo apt-get install curl unzip.
# - If the upstream URL changes, override with env YOLOV8N_ONNX_URL.
set -euo pipefail

MODEL_DIR="${MODEL_DIR:-models}"
MODEL_PATH="${MODEL_DIR}/yolov8n.onnx"
mkdir -p "${MODEL_DIR}"

if [[ -f "${MODEL_PATH}" ]]; then
    SIZE=$(stat -c%s "${MODEL_PATH}" 2>/dev/null || stat -f%z "${MODEL_PATH}")
    echo "[download_models] ${MODEL_PATH} already present (${SIZE} bytes), skipping."
    exit 0
fi

# Default: Ultralytics-hosted YOLOv8n ONNX (COCO, 80 classes, 640 input).
YOLOV8N_ONNX_URL="${YOLOV8N_ONNX_URL:-https://github.com/ultralytics/assets/releases/download/v8.3.0/yolov8n.onnx}"

echo "[download_models] Downloading ${YOLOV8N_ONNX_URL}"
echo "[download_models] -> ${MODEL_PATH}"
curl -L --fail --show-error --progress-bar -o "${MODEL_PATH}" "${YOLOV8N_ONNX_URL}"

SIZE=$(stat -c%s "${MODEL_PATH}" 2>/dev/null || stat -f%z "${MODEL_PATH}")
echo "[download_models] OK: ${MODEL_PATH} (${SIZE} bytes)"
echo ""
echo "Next: enable the cpu-onnx feature and run an inference example:"
echo "  cargo run -p cv-inference --example onnx_infer --features cpu-onnx -- \\"
echo "      ${MODEL_PATH} path/to/image.jpg"
