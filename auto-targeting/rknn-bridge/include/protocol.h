// rknn-bridge: shared protocol definitions between C++ and Rust.
//
// This header is the single source of truth for the IPC protocol between
// the Rust orchestrator (cv-inference crate) and the C++ rknn-bridge
// microservice. See docs/ADR/0001-rknn-cpp-bridge.md for the full spec.

#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace rknn_bridge {

// === Bounding box ===
struct BoundingBox {
    uint32_t x;
    uint32_t y;
    uint32_t width;
    uint32_t height;
};

// === Detection (one per object found in a frame) ===
struct Detection {
    BoundingBox bbox;
    std::string class_name;
    uint32_t class_id;
    float confidence;
    uint64_t frame_seq;
    uint64_t detected_at_ms;  // Unix epoch milliseconds
};

// === Init message (Rust → Bridge) ===
struct InitRequest {
    std::string model_path;
    uint32_t input_width;
    uint32_t input_height;
    std::string input_format;  // "nv12", "rgb24", etc.
    float confidence_threshold;
    float nms_threshold;
};

struct InitResponse {
    bool ok;
    std::string error;
    uint32_t output_classes;
    std::string backend;  // "rknn" or "stub"
};

// === Infer message (Rust → Bridge) ===
// Frame data is passed via shared memory (memfd), not the socket.
struct InferRequest {
    uint64_t frame_seq;
    int shm_fd;          // file descriptor passed via SCM_RIGHTS
    size_t shm_size;     // size of the shared memory region
    uint64_t captured_at_ms;
};

struct InferResponse {
    bool ok;
    std::string error;
    uint64_t frame_seq;
    uint32_t latency_ms;
    std::vector<Detection> detections;
};

// === Health check ===
struct HealthResponse {
    bool ok;
    bool model_loaded;
    float npu_utilization;  // [0.0, 1.0], or -1 if unavailable
    std::string backend;
};

// === Message types (for the JSON envelope on the Unix socket) ===
enum class MessageType : uint8_t {
    INIT = 1,
    INIT_ACK = 2,
    INFER = 3,
    INFER_ACK = 4,
    HEALTH = 5,
    HEALTH_ACK = 6,
    SHUTDOWN = 7,
    SHUTDOWN_ACK = 8,
};

// JSON serialization helpers (implemented in bridge_main.cpp)
std::string serialize_init_request(const InitRequest& req);
bool parse_init_request(const std::string& json, InitRequest& req);

std::string serialize_infer_response(const InferResponse& resp);
bool parse_infer_request(const std::string& json, InferRequest& req);

std::string serialize_health_response(const HealthResponse& resp);

}  // namespace rknn_bridge
