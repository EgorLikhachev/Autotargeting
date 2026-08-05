// rknn_model.cpp — Inference backend implementations.
//
// HAVE_RKNN=1: real RKNN backend using librknnrt.so.
// HAVE_RKNN=0: stub backend that returns fake detections.

#include "rknn_model.h"
#include "nms.h"

#include <algorithm>
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

        // Initialize RKNN. Flags=0 + rknn_init_extension nullptr. We bind to a
        // single NPU core post-init (mirrors rknn-toolkit2's init_runtime with
        // core_mask=NPU_CORE_0) — multi-core is not needed for a single stream.
        int ret = rknn_init(&ctx_, model_data.data(), size, 0, nullptr);
        if (ret < 0) {
            error = "rknn_init failed: " + std::to_string(ret);
            return false;
        }

        // Bind to NPU core 0 (matches rknn-toolkit2 init_runtime default).
        // Without this the runtime could schedule on AUTO and the subsequent
        // rknn_outputs_get has been observed to segfault on driver 2.3.0.
        ret = rknn_set_core_mask(ctx_, RKNN_NPU_CORE_0);
        if (ret < 0) {
            std::cerr << "[RknnBackend] rknn_set_core_mask failed: " << ret
                      << " (continuing with default)\n";
        }

        // Query model attributes
        rknn_input_output_num io_num;
        ret = rknn_query(ctx_, RKNN_QUERY_IN_OUT_NUM, &io_num, sizeof(io_num));
        if (ret < 0) {
            error = "rknn_query failed: " + std::to_string(ret);
            return false;
        }

        // Query the first output tensor's shape so we can parse YOLOv8 output
        // without hard-coding the class count / anchor count. The standard
        // Ultralytics YOLOv8 RKNN export has one output of shape [1, 4+nc, A]
        // where A = 8400 for 640x640 input (80² + 40² + 20²).
        if (io_num.n_output >= 1) {
            rknn_tensor_attr out_attr;
            memset(&out_attr, 0, sizeof(out_attr));
            out_attr.index = 0;
            ret = rknn_query(ctx_, RKNN_QUERY_OUTPUT_ATTR, &out_attr, sizeof(out_attr));
            if (ret < 0) {
                error = "rknn_query OUTPUT_ATTR failed: " + std::to_string(ret);
                return false;
            }
            // YOLOv8 output is [1, 4+nc, A] (n_dims==3, CHW). Some RKNN exports
            // transpose to [1, A, 4+nc]; we detect both via n_dims / dims.
            if (out_attr.n_dims == 3) {
                // dims are stored in either HWC or CHW order depending on fmt;
                // for the canonical YOLOv8 export fmt=NF (float) CHW: dims = [1, 4+nc, A].
                // Pick the two non-1 entries as (rows, anchors).
                uint32_t vals[3] = {out_attr.dims[0], out_attr.dims[1], out_attr.dims[2]};
                // Sort descending so vals[0] is the largest (anchors, usually 8400),
                // vals[1] is the row count (4+nc), vals[2] is 1.
                std::sort(vals, vals + 3, std::greater<uint32_t>());
                if (vals[2] == 1 && vals[1] >= 5) {
                    output_rows_ = vals[1];      // 4 + nc
                    output_anchors_ = vals[0];   // A (8400 for 640 input)
                }
            }
        }

        loaded_ = true;
        std::cerr << "[RknnBackend] Loaded model " << model_path
                  << " (input=" << input_width << "x" << input_height
                  << ", inputs=" << io_num.n_input << ", outputs=" << io_num.n_output
                  << ", out_rows=" << output_rows_
                  << ", out_anchors=" << output_anchors_ << ")\n";
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

        // Set input.
        //
        // RKNN SDK 2.x renamed the pixel-layout enum: the old
        // RKNN_TENSOR_FORMAT_RGB (SDK 1.x) is gone. RGB24 packed bytes are
        // now described as NHWC layout (N=1 implicit, H=height, W=width,
        // C=3 channels). The model itself defines the channel order (RGB vs
        // BGR) at conversion time; here we only declare the memory layout.
        rknn_input input;
        memset(&input, 0, sizeof(input));
        input.index = 0;
        input.type = RKNN_TENSOR_UINT8;
        input.size = input_width_ * input_height_ * 3;  // RGB24 packed
        input.buf = const_cast<uint8_t*>(frame_data);
        input.fmt = RKNN_TENSOR_NHWC;

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

        // Get outputs.
        //
        // The standard Ultralytics YOLOv8 RKNN export emits a single output
        // tensor of shape [1, 4+nc, A] (row-major, float32), where:
        //   - row 0..3: cx, cy, w, h (in 640x640 letterbox space)
        //   - row 4..4+nc: per-class probabilities (already after sigmoid in
        //     the canonical export — raw values ARE probabilities in [0,1])
        // A = 8400 for a 640x640 input (80² + 40² + 20²).
        //
        // This is the exact same numeric logic as the pure-Rust parser in
        // crates/yolov8/src/lib.rs (postprocess). Keep them in sync — the
        // Rust CPU path and the C++ NPU path must produce identical boxes.
        if (output_rows_ < 5 || output_anchors_ == 0) {
            std::cerr << "[RknnBackend] output shape not queried; cannot parse\n";
            return dets;
        }

        rknn_output output;
        memset(&output, 0, sizeof(output));
        output.want_float = 0;     // read NATIVE dtype (rknn-toolkit2 2.3.0 segfaults
                                   // on want_float=1 with int8 models during outputs_get)
        output.is_prealloc = 0;

        ret = rknn_outputs_get(ctx_, 0, &output, nullptr);
        if (ret < 0) {
            std::cerr << "[RknnBackend] rknn_outputs_get failed: " << ret << "\n";
            return dets;
        }

        // Re-query output attrs to learn the native dtype + qnt (scale/zero-point)
        // so we can dequantize int8 -> float ourselves if needed.
        rknn_tensor_attr out_attr;
        memset(&out_attr, 0, sizeof(out_attr));
        out_attr.index = 0;
        rknn_query(ctx_, RKNN_QUERY_OUTPUT_ATTR, &out_attr, sizeof(out_attr));
        std::cerr << "[RknnBackend] out: size=" << output.size
                  << " type=" << out_attr.type
                  << " fl=" << out_attr.fmt
                  << " qnt_type=" << out_attr.qnt_type
                  << " scale=" << out_attr.scale
                  << " zp=" << out_attr.zp
                  << " n_dims=" << out_attr.n_dims << "\n";

        const size_t expected_elems =
            static_cast<size_t>(output_rows_) * static_cast<size_t>(output_anchors_);

        // Dequantize to float regardless of native dtype (int8/float32).
        // RKNN-TENSOR-TYPE: 1=float32, 2=int8 (per rknn_api.h RKNN_TENSOR_TYPE_).
        std::vector<float> floats;
        floats.reserve(expected_elems);
        if (out_attr.type == 1 /*RKNN_TENSOR_FLOAT32*/) {
            if (output.size < expected_elems * sizeof(float)) {
                std::cerr << "[RknnBackend] float output too small: " << output.size << "\n";
                rknn_outputs_release(ctx_, 1, &output);
                return dets;
            }
            const float* p = static_cast<const float*>(output.buf);
            floats.assign(p, p + expected_elems);
        } else {
            // int8 (or other) — dequantize via scale/zero-point.
            const int8_t* p = static_cast<const int8_t*>(output.buf);
            const float scale = out_attr.scale;
            const int32_t zp = out_attr.zp;
            for (size_t i = 0; i < expected_elems; ++i) {
                floats.push_back((static_cast<float>(p[i]) - zp) * scale);
            }
        }

        const uint32_t rows = output_rows_;
        const uint32_t anchors = output_anchors_;
        const uint32_t nc = rows - 4;

        // Letterbox reverse-transform params (must match the Rust preprocessor).
        // The Rust side feeds a letterboxed 640x640 RGB frame; here we map back
        // to the ORIGINAL frame size == input_width_ x input_height_. (When the
        // caller letterboxes upstream, input_width_/height_ already reflect the
        // original frame; the NPU sees a pre-letterboxed buffer.)
        const float orig_w = static_cast<float>(input_width_);
        const float orig_h = static_cast<float>(input_height_);
        const float target = 640.0f;
        float scale = std::min(target / orig_w, target / orig_h);
        if (scale <= 0.0f || !std::isfinite(scale)) scale = 1.0f;
        const float new_w = orig_w * scale;
        const float new_h = orig_h * scale;
        const float pad_x = (target - new_w) * 0.5f;
        const float pad_y = (target - new_h) * 0.5f;
        const float inv_scale = (scale > 0.0f) ? (1.0f / scale) : 0.0f;

        // 1) Sweep anchors, pick best class per anchor, threshold.
        struct Cand {
            float cx, cy, w, h;
            uint32_t class_id;
            float conf;
        };
        std::vector<Cand> cands;
        cands.reserve(256);
        const float* out = floats.data();
        for (uint32_t a = 0; a < anchors; ++a) {
            const float cx = out[0 * anchors + a];
            const float cy = out[1 * anchors + a];
            const float w = out[2 * anchors + a];
            const float h = out[3 * anchors + a];

            uint32_t best_id = 0;
            float best_score = -INFINITY;
            for (uint32_t c = 0; c < nc; ++c) {
                const float s = out[(4 + c) * anchors + a];
                if (s > best_score) {
                    best_score = s;
                    best_id = c;
                }
            }
            float conf = best_score;
            if (conf < 0.0f) conf = 0.0f;
            if (conf > 1.0f) conf = 1.0f;
            if (conf < confidence_threshold_) continue;
            // Skip degenerate boxes.
            if (!std::isfinite(cx) || !std::isfinite(cy) || w <= 0.0f || h <= 0.0f) continue;
            cands.push_back({cx, cy, w, h, best_id, conf});
        }

        // 2) Sort by confidence descending.
        std::sort(cands.begin(), cands.end(),
                  [](const Cand& x, const Cand& y) { return x.conf > y.conf; });

        // 3) Greedy NMS in 640-space (IoU is invariant under letterbox).
        const uint64_t now_ms = std::chrono::duration_cast<std::chrono::milliseconds>(
            std::chrono::system_clock::now().time_since_epoch()).count();

        std::vector<char> suppressed(cands.size(), 0);
        for (size_t i = 0; i < cands.size(); ++i) {
            if (suppressed[i]) continue;
            const Cand& ci = cands[i];
            for (size_t j = i + 1; j < cands.size(); ++j) {
                if (suppressed[j]) continue;
                const Cand& cj = cands[j];
                const float ax1 = ci.cx - ci.w * 0.5f, ay1 = ci.cy - ci.h * 0.5f;
                const float ax2 = ci.cx + ci.w * 0.5f, ay2 = ci.cy + ci.h * 0.5f;
                const float bx1 = cj.cx - cj.w * 0.5f, by1 = cj.cy - cj.h * 0.5f;
                const float bx2 = cj.cx + cj.w * 0.5f, by2 = cj.cy + cj.h * 0.5f;
                const float ix1 = std::max(ax1, bx1), iy1 = std::max(ay1, by1);
                const float ix2 = std::min(ax2, bx2), iy2 = std::min(ay2, by2);
                const float iw = std::max(0.0f, ix2 - ix1);
                const float ih = std::max(0.0f, iy2 - iy1);
                const float inter = iw * ih;
                const float uni = ci.w * ci.h + cj.w * cj.h - inter;
                const float iou = (uni > 0.0f) ? (inter / uni) : 0.0f;
                if (iou > nms_threshold_) suppressed[j] = 1;
            }

            // 4) Map back to original frame coordinates.
            Detection d;
            const float ox = (ci.cx - pad_x) * inv_scale;
            const float oy = (ci.cy - pad_y) * inv_scale;
            const float bw = ci.w * inv_scale;
            const float bh = ci.h * inv_scale;
            const float x0 = std::max(0.0f, std::min(orig_w, ox - bw * 0.5f));
            const float y0 = std::max(0.0f, std::min(orig_h, oy - bh * 0.5f));
            const float x1 = std::max(0.0f, std::min(orig_w, ox + bw * 0.5f));
            const float y1 = std::max(0.0f, std::min(orig_h, oy + bh * 0.5f));
            d.bbox.x = static_cast<uint32_t>(std::round(x0));
            d.bbox.y = static_cast<uint32_t>(std::round(y0));
            d.bbox.width = static_cast<uint32_t>(std::max(1u, static_cast<uint32_t>(std::round(x1 - x0))));
            d.bbox.height = static_cast<uint32_t>(std::max(1u, static_cast<uint32_t>(std::round(y1 - y0))));
            d.class_id = ci.class_id;
            d.class_name = coco_label(ci.class_id);
            d.confidence = ci.conf;
            d.frame_seq = frame_seq;
            d.detected_at_ms = now_ms;
            dets.push_back(d);
        }

        rknn_outputs_release(ctx_, 1, &output);
        return dets;
    }

    std::string backend_name() const override { return "rknn"; }
    uint32_t output_classes() const override { return 80; }
    bool is_loaded() const override { return loaded_; }
    float npu_utilization() const override { return -1.0f; }

private:
    // COCO 80-class label table — same order as crates/yolov8::COCO_LABELS.
    // Used so detections carry a human-readable class_name without needing
    // an external labels file on the device.
    static const char* coco_label(uint32_t class_id) {
        static constexpr const char* kCoco[80] = {
            "person", "bicycle", "car", "motorcycle", "airplane", "bus", "train",
            "truck", "boat", "traffic light", "fire hydrant", "stop sign",
            "parking meter", "bench", "bird", "cat", "dog", "horse", "sheep",
            "cow", "elephant", "bear", "zebra", "giraffe", "backpack", "umbrella",
            "handbag", "tie", "suitcase", "frisbee", "skis", "snowboard",
            "sports ball", "kite", "baseball bat", "baseball glove", "skateboard",
            "surfboard", "tennis racket", "bottle", "wine glass", "cup", "fork",
            "knife", "spoon", "bowl", "banana", "apple", "sandwich", "orange",
            "broccoli", "carrot", "hot dog", "pizza", "donut", "cake", "chair",
            "couch", "potted plant", "bed", "dining table", "toilet", "tv",
            "laptop", "mouse", "remote", "keyboard", "cell phone", "microwave",
            "oven", "toaster", "sink", "refrigerator", "book", "clock", "vase",
            "scissors", "teddy bear", "hair drier", "toothbrush"};
        if (class_id < 80) return kCoco[class_id];
        return "unknown";
    }

    rknn_context ctx_ = -1;
    uint32_t input_width_ = 0;
    uint32_t input_height_ = 0;
    std::string input_format_;
    float confidence_threshold_ = 0.45f;
    float nms_threshold_ = 0.45f;
    // Queried from the model at load time: output is [1, output_rows_, output_anchors_].
    // output_rows_ = 4 + nc (e.g. 84 for COCO). output_anchors_ = 8400 for 640 input.
    uint32_t output_rows_ = 0;
    uint32_t output_anchors_ = 0;
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
