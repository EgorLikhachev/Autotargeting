// rknn_model.cpp — Inference backend implementations.
//
// HAVE_RKNN=1: real RKNN backend using librknnrt.so.
// HAVE_RKNN=0: stub backend that returns fake detections.

#include "rknn_model.h"
#include "nms.h"

#include <chrono>
#include <cmath>
#include <cstring>
#include <iostream>

#if HAVE_RKNN
#include "rknn_api.h"
#endif

namespace rknn_bridge {

// ============================================================
// Stub backend — returns fake detections for testing without NPU.
// ============================================================
class StubBackend : public InferenceBackend {
public:
    bool load_model(const std::string& model_path,
                    uint32_t input_width,
                    uint32_t input_height,
                    const std::string& input_format,
                    float confidence_threshold,
                    float nms_threshold,
                    std::string& error) override {
        (void)model_path;  // unused in stub
        input_width_ = input_width;
        input_height_ = input_height;
        input_format_ = input_format;
        confidence_threshold_ = confidence_threshold;
        nms_threshold_ = nms_threshold;
        loaded_ = true;
        error.clear();
        std::cerr << "[StubBackend] Loaded (stub) — input " << input_width << "x"
                  << input_height << " " << input_format
                  << ", conf_thresh=" << confidence_threshold
                  << ", nms_thresh=" << nms_threshold << "\n";
        return true;
    }

    std::vector<Detection> infer(const uint8_t* frame_data,
                                 size_t frame_size,
                                 uint64_t frame_seq) override {
        (void)frame_size;

        std::vector<Detection> dets;

        if (!loaded_) {
            return dets;
        }

        // Generate 1-3 fake detections based on frame brightness.
        // This is deterministic based on frame_seq so tests are reproducible.
        uint64_t seed = frame_seq;

        // Detection 1: a "person" in the center-left
        Detection d1;
        d1.bbox.x = static_cast<uint32_t>(input_width_ * 0.3);
        d1.bbox.y = static_cast<uint32_t>(input_height_ * 0.4);
        d1.bbox.width = 60;
        d1.bbox.height = 120;
        d1.class_name = "person";
        d1.class_id = 0;
        d1.confidence = 0.85f + 0.1f * (seed % 10) / 10.0f;
        d1.frame_seq = frame_seq;
        d1.detected_at_ms = std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::system_clock::now().time_since_epoch()).count();
        if (d1.confidence >= confidence_threshold_) {
            dets.push_back(d1);
        }

        // Detection 2: occasionally a "vehicle" in the right
        if (seed % 3 == 0) {
            Detection d2;
            d2.bbox.x = static_cast<uint32_t>(input_width_ * 0.7);
            d2.bbox.y = static_cast<uint32_t>(input_height_ * 0.5);
            d2.bbox.width = 100;
            d2.bbox.height = 80;
            d2.class_name = "vehicle";
            d2.class_id = 2;
            d2.confidence = 0.75f;
            d2.frame_seq = frame_seq;
            d2.detected_at_ms = d1.detected_at_ms;
            if (d2.confidence >= confidence_threshold_) {
                dets.push_back(d2);
            }
        }

        // Apply NMS
        non_max_suppression(dets, nms_threshold_);

        return dets;
    }

    std::string backend_name() const override { return "stub"; }
    uint32_t output_classes() const override { return 80; }  // COCO classes
    bool is_loaded() const override { return loaded_; }
    float npu_utilization() const override { return -1.0f; }  // unavailable

private:
    uint32_t input_width_ = 0;
    uint32_t input_height_ = 0;
    std::string input_format_;
    float confidence_threshold_ = 0.45f;
    float nms_threshold_ = 0.45f;
    bool loaded_ = false;
};

#if HAVE_RKNN
// ============================================================
// Real RKNN backend using librknnrt.so.
// ============================================================
class RknnBackend : public InferenceBackend {
public:
    ~RknnBackend() override {
        if (ctx_ >= 0) {
            rknn_destroy(ctx_);
        }
    }

    bool load_model(const std::string& model_path,
                    uint32_t input_width,
                    uint32_t input_height,
                    const std::string& input_format,
                    float confidence_threshold,
                    float nms_threshold,
                    std::string& error) override {
        input_width_ = input_width;
        input_height_ = input_height;
        input_format_ = input_format;
        confidence_threshold_ = confidence_threshold;
        nms_threshold_ = nms_threshold;

        // Read the model file
        FILE* fp = fopen(model_path.c_str(), "rb");
        if (!fp) {
            error = "failed to open model file: " + model_path;
            return false;
        }
        fseek(fp, 0, SEEK_END);
        size_t size = ftell(fp);
        fseek(fp, 0, SEEK_SET);
        std::vector<uint8_t> model_data(size);
        if (fread(model_data.data(), 1, size, fp) != size) {
            error = "failed to read model file";
            fclose(fp);
            return false;
        }
        fclose(fp);

        // Initialize RKNN
        int ret = rknn_init(&ctx_, model_data.data(), size, 0, nullptr);
        if (ret < 0) {
            error = "rknn_init failed: " + std::to_string(ret);
            return false;
        }

        // Query model attributes
        rknn_input_output_num io_num;
        ret = rknn_query(ctx_, RKNN_QUERY_IN_OUT_NUM, &io_num, sizeof(io_num));
        if (ret < 0) {
            error = "rknn_query failed: " + std::to_string(ret);
            return false;
        }

        loaded_ = true;
        std::cerr << "[RknnBackend] Loaded model " << model_path
                  << " (input=" << input_width << "x" << input_height
                  << ", inputs=" << io_num.n_input << ", outputs=" << io_num.n_output << ")\n";
        return true;
    }

    std::vector<Detection> infer(const uint8_t* frame_data,
                                 size_t frame_size,
                                 uint64_t frame_seq) override {
        (void)frame_size;
        std::vector<Detection> dets;
        if (!loaded_) {
            return dets;
        }

        // Set input
        rknn_input input;
        memset(&input, 0, sizeof(input));
        input.index = 0;
        input.type = RKNN_TENSOR_UINT8;
        input.size = input_width_ * input_height_ * 3;  // RGB24
        input.buf = const_cast<uint8_t*>(frame_data);
        input.fmt = RKNN_TENSOR_FORMAT_RGB;

        int ret = rknn_inputs_set(ctx_, 1, &input);
        if (ret < 0) {
            std::cerr << "[RknnBackend] rknn_inputs_set failed: " << ret << "\n";
            return dets;
        }

        // Run inference
        ret = rknn_run(ctx_, nullptr);
        if (ret < 0) {
            std::cerr << "[RknnBackend] rknn_run failed: " << ret << "\n";
            return dets;
        }

        // Get outputs
        // TODO: parse YOLOv8 output format properly. For now, return empty.
        // This requires knowing the model's output tensor layout.

        return dets;
    }

    std::string backend_name() const override { return "rknn"; }
    uint32_t output_classes() const override { return 80; }
    bool is_loaded() const override { return loaded_; }
    float npu_utilization() const override { return -1.0f; }

private:
    rknn_context ctx_ = -1;
    uint32_t input_width_ = 0;
    uint32_t input_height_ = 0;
    std::string input_format_;
    float confidence_threshold_ = 0.45f;
    float nms_threshold_ = 0.45f;
    bool loaded_ = false;
};
#endif  // HAVE_RKNN

// ============================================================
// Factory
// ============================================================
std::unique_ptr<InferenceBackend> create_backend() {
#if HAVE_RKNN
    return std::make_unique<RknnBackend>();
#else
    return std::make_unique<StubBackend>();
#endif
}

}  // namespace rknn_bridge
