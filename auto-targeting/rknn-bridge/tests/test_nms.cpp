// test_nms.cpp — unit tests for the NMS implementation.

#include "../include/nms.h"

#include <cassert>
#include <iostream>
#include <vector>

using namespace rknn_bridge;

static BoundingBox make_bbox(uint32_t x, uint32_t y, uint32_t w, uint32_t h) {
    return {x, y, w, h};
}

static Detection make_det(uint32_t x, uint32_t y, uint32_t w, uint32_t h, float conf) {
    Detection d;
    d.bbox = make_bbox(x, y, w, h);
    d.class_name = "person";
    d.class_id = 0;
    d.confidence = conf;
    d.frame_seq = 1;
    d.detected_at_ms = 0;
    return d;
}

static void test_iou_identical() {
    auto b = make_bbox(10, 10, 50, 50);
    float score = iou(b, b);
    assert(score > 0.99f);
    std::cout << "test_iou_identical: PASS\n";
}

static void test_iou_disjoint() {
    auto a = make_bbox(0, 0, 50, 50);
    auto b = make_bbox(100, 100, 50, 50);
    float score = iou(a, b);
    assert(score == 0.0f);
    std::cout << "test_iou_disjoint: PASS\n";
}

static void test_iou_partial() {
    auto a = make_bbox(0, 0, 100, 100);
    auto b = make_bbox(50, 50, 100, 100);
    float score = iou(a, b);
    // Intersection: 50x50 = 2500; Union: 10000 + 10000 - 2500 = 17500
    // IoU = 2500/17500 ≈ 0.143
    assert(score > 0.14f && score < 0.15f);
    std::cout << "test_iou_partial: PASS\n";
}

static void test_nms_keeps_highest() {
    std::vector<Detection> dets = {
        make_det(100, 100, 50, 50, 0.9),
        make_det(105, 105, 50, 50, 0.7),  // overlaps heavily with first
        make_det(500, 500, 50, 50, 0.8),  // disjoint
    };
    non_max_suppression(dets, 0.45f);
    assert(dets.size() == 2);
    assert(dets[0].confidence == 0.9f);
    assert(dets[1].confidence == 0.8f);
    std::cout << "test_nms_keeps_highest: PASS\n";
}

static void test_nms_empty() {
    std::vector<Detection> dets;
    non_max_suppression(dets, 0.45f);
    assert(dets.empty());
    std::cout << "test_nms_empty: PASS\n";
}

static void test_nms_disjoint_all_kept() {
    std::vector<Detection> dets = {
        make_det(0, 0, 50, 50, 0.9),
        make_det(200, 200, 50, 50, 0.8),
        make_det(400, 400, 50, 50, 0.7),
    };
    non_max_suppression(dets, 0.45f);
    assert(dets.size() == 3);
    std::cout << "test_nms_disjoint_all_kept: PASS\n";
}

int main() {
    test_iou_identical();
    test_iou_disjoint();
    test_iou_partial();
    test_nms_keeps_highest();
    test_nms_empty();
    test_nms_disjoint_all_kept();

    std::cout << "\nAll tests passed!\n";
    return 0;
}
