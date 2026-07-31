// rknn_model.h — abstraction over the RKNN SDK.
//
// When HAVE_RKNN=1 (real NPU available), this loads a model via librknnrt.so.
// When HAVE_RKNN=0 (no NPU), this is a stub that returns fake detections.

#pragma once

#include "protocol.h"
#include <memory>
#include <string>

namespace rknn_bridge {

// Abstract interface — implemented by RknnBackend (real) or StubBackend.
class InferenceBackend {
public:
    virtual ~InferenceBackend() = default;

    // Load a model from the given .rknn file path.
    // Returns true on success, false on failure (sets `error`).
    virtual bool load_model(const std::string& model_path,
                           uint32_t input_width,
                           uint32_t input_height,
                           const std::string& input_format,
                           float confidence_threshold,
                           float nms_threshold,
                           std::string& error) = 0;

    // Run inference on a frame. `frame_data` is raw pixel data in the
    // format specified at load_model() time.
    // `frame_seq` is the frame sequence number (for diagnostics).
    // Returns a list of detections (already NMS-filtered).
    virtual std::vector<Detection> infer(const uint8_t* frame_data,
                                         size_t frame_size,
                                         uint64_t frame_seq) = 0;

    // Backend name ("rknn" or "stub").
    virtual std::string backend_name() const = 0;

    // Number of output classes the model produces.
    virtual uint32_t output_classes() const = 0;

    // Whether the model is loaded and ready.
    virtual bool is_loaded() const = 0;

    // NPU utilization in [0.0, 1.0], or -1.0 if unavailable.
    virtual float npu_utilization() const = 0;
};

// Factory: returns a real RknnBackend if HAVE_RKNN=1, else a StubBackend.
std::unique_ptr<InferenceBackend> create_backend();

}  // namespace rknn_bridge
