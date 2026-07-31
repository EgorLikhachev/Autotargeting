// nms.h — Non-Maximum Suppression for C++ side.
//
// Same algorithm as the Rust version (crates/cv-inference/src/nms.rs),
// but implemented in C++ for use within the rknn-bridge microservice.

#pragma once

#include "protocol.h"
#include <vector>

namespace rknn_bridge {

// Run NMS on a list of detections in-place.
// Detections with IoU > `iou_threshold` are removed, keeping the one with
// the highest confidence.
void non_max_suppression(std::vector<Detection>& detections, float iou_threshold);

// Compute IoU between two bounding boxes.
float iou(const BoundingBox& a, const BoundingBox& b);

}  // namespace rknn_bridge
