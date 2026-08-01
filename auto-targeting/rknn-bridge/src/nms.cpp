// nms.cpp — Non-Maximum Suppression implementation.
//
// Same algorithm as crates/cv-inference/src/nms.rs.

#include "nms.h"
#include <algorithm>

namespace rknn_bridge {

float iou(const BoundingBox& a, const BoundingBox& b) {
    uint32_t x1 = std::max(a.x, b.x);
    uint32_t y1 = std::max(a.y, b.y);
    uint32_t x2 = std::min(a.x + a.width, b.x + b.width);
    uint32_t y2 = std::min(a.y + a.height, b.y + b.height);

    if (x2 <= x1 || y2 <= y1) {
        return 0.0f;
    }

    float intersection = static_cast<float>(x2 - x1) * static_cast<float>(y2 - y1);
    float union_area =
        static_cast<float>(a.width * a.height + b.width * b.height) - intersection;

    if (union_area <= 0.0f) {
        return 0.0f;
    }
    return intersection / union_area;
}

void non_max_suppression(std::vector<Detection>& detections, float iou_threshold) {
    if (detections.empty()) {
        return;
    }

    // Sort by confidence descending
    std::sort(detections.begin(), detections.end(),
              [](const Detection& a, const Detection& b) {
                  return a.confidence > b.confidence;
              });

    std::vector<Detection> keep;
    std::vector<bool> suppressed(detections.size(), false);

    for (size_t i = 0; i < detections.size(); ++i) {
        if (suppressed[i]) {
            continue;
        }
        keep.push_back(detections[i]);
        for (size_t j = i + 1; j < detections.size(); ++j) {
            if (suppressed[j]) {
                continue;
            }
            float score = iou(detections[i].bbox, detections[j].bbox);
            if (score > iou_threshold) {
                suppressed[j] = true;
            }
        }
    }

    detections = std::move(keep);
}

}  // namespace rknn_bridge
