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
        // Release zero-copy tensor memory BEFORE the context is destroyed.
        // Order matters: rknn_destroy_mem needs a live ctx_.
        if (ctx_ >= 0) {
            if (output_mem_) rknn_destroy_mem(ctx_, output_mem_);
            if (input_mem_) rknn_destroy_mem(ctx_, input_mem_);
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

        // Log SDK version — critical for diagnosing header/library mismatches.
        rknn_sdk_version sdk_ver;
        memset(&sdk_ver, 0, sizeof(sdk_ver));
        if (rknn_query(ctx_, RKNN_QUERY_SDK_VERSION, &sdk_ver, sizeof(sdk_ver)) == 0) {
            std::cerr << "[RknnBackend] SDK api=" << sdk_ver.api_version
                      << " drv=" << sdk_ver.drv_version << "\n";
        }

        // Query the INPUT tensor attrs and store in input_attr_ (used for
        // zero-copy set_io_mem below and memcpy in infer). We FORCE the input
        // type to UINT8 + fmt NHWC — the NPU then fuses mean/std normalization
        // and quantization internally (matches how we feed RGB24 packed bytes).
        memset(&input_attr_, 0, sizeof(input_attr_));
        input_attr_.index = 0;
        ret = rknn_query(ctx_, RKNN_QUERY_INPUT_ATTR, &input_attr_, sizeof(input_attr_));
        if (ret < 0) {
            error = "rknn_query INPUT_ATTR failed: " + std::to_string(ret);
            return false;
        }
        input_attr_.type = RKNN_TENSOR_UINT8;
        input_attr_.fmt = RKNN_TENSOR_NHWC;
        std::cerr << "[RknnBackend] input: type=" << input_attr_.type
                  << " fmt=" << input_attr_.fmt
                  << " n_dims=" << input_attr_.n_dims
                  << " dims=[" << input_attr_.dims[0];
        for (uint32_t i = 1; i < input_attr_.n_dims && i < 16; ++i) {
            std::cerr << "," << input_attr_.dims[i];
        }
        std::cerr << "] size=" << input_attr_.size
                  << " size_with_stride=" << input_attr_.size_with_stride << "\n";

        // Query the OUTPUT tensor attrs and store in output_attr_.
        // The standard Ultralytics YOLOv8 RKNN export has one output of shape
        // [1, 4+nc, A] where A = 8400 for 640x640 input (80² + 40² + 20²).
        memset(&output_attr_, 0, sizeof(output_attr_));
        output_attr_.index = 0;
        ret = rknn_query(ctx_, RKNN_QUERY_OUTPUT_ATTR, &output_attr_, sizeof(output_attr_));
        if (ret < 0) {
            error = "rknn_query OUTPUT_ATTR failed: " + std::to_string(ret);
            return false;
        }
        // Detect (4+nc, anchors) from dims regardless of layout (CHW or HWC).
        if (output_attr_.n_dims == 3) {
            uint32_t vals[3] = {output_attr_.dims[0], output_attr_.dims[1], output_attr_.dims[2]};
            std::sort(vals, vals + 3, std::greater<uint32_t>());
            if (vals[2] == 1 && vals[1] >= 5) {
                output_rows_ = vals[1];      // 4 + nc
                output_anchors_ = vals[0];   // A (8400 for 640 input)
            }
        }
        // is_quant_ distinguishes int8 models (need int8 dequant) from float16
        // (we request FLOAT32 output via attr.type, runtime converts).
        // rknn_tensor_type enum: 0=float32, 1=float16, 2=int8, 3=uint8.
        is_quant_ = (output_attr_.qnt_type == 1 /*RKNN_TENSOR_QNT_AFFINE_ASYMMETRIC*/
                     && output_attr_.type == 2  /*RKNN_TENSOR_INT8*/);
        // FORCE the output type to FLOAT32 — this is the key trick from the
        // official rknn_create_mem_demo.cpp: by setting attr.type before
        // rknn_set_io_mem, we ask the runtime to convert native NPU output
        // (fp16 or int8) into a float32 buffer that we can read directly.
        output_attr_.type = RKNN_TENSOR_FLOAT32;
        std::cerr << "[RknnBackend] output: type=" << output_attr_.type
                  << " n_dims=" << output_attr_.n_dims
                  << " n_elems=" << output_attr_.n_elems
                  << " size=" << output_attr_.size
                  << " is_quant=" << is_quant_ << "\n";

        // === Allocate persistent zero-copy tensor memory (once, reused per frame).
        // This bypasses rknn_inputs_set / rknn_outputs_get entirely — the NPU
        // reads input and writes output directly into our buffers, and the
        // attr.type override above makes it deliver float32 output.
        input_mem_ = rknn_create_mem(ctx_, input_attr_.size_with_stride);
        if (!input_mem_) {
            error = "rknn_create_mem(input) failed";
            return false;
        }
        ret = rknn_set_io_mem(ctx_, input_mem_, &input_attr_);
        if (ret < 0) {
            error = "rknn_set_io_mem(input) failed: " + std::to_string(ret);
            return false;
        }
        const uint32_t out_bytes = output_attr_.n_elems * sizeof(float);
        output_mem_ = rknn_create_mem(ctx_, out_bytes);
        if (!output_mem_) {
            error = "rknn_create_mem(output) failed";
            return false;
        }
        ret = rknn_set_io_mem(ctx_, output_mem_, &output_attr_);
        if (ret < 0) {
            error = "rknn_set_io_mem(output) failed: " + std::to_string(ret);
            return false;
        }

        loaded_ = true;
        std::cerr << "[RknnBackend] Loaded model " << model_path
                  << " (input=" << input_width << "x" << input_height
                  << ", inputs=" << io_num.n_input << ", outputs=" << io_num.n_output
                  << ", out_rows=" << output_rows_
                  << ", out_anchors=" << output_anchors_
                  << ", io=zero-copy)\n";
        return true;
    }

    std::vector<Detection> infer(const uint8_t* frame_data,
                                 size_t frame_size,
                                 uint64_t frame_seq) override {
        (void)frame_size;
        std::vector<Detection> dets;
        if (!loaded_ || !input_mem_ || !output_mem_) {
            return dets;
        }

        // Copy the RGB24 packed frame into the zero-copy input tensor buffer.
        // input_attr_ was set to UINT8/NHWC at load; size_with_stride accounts
        // for NPU row alignment. For a 640x640x3 input the stride usually
        // equals the row width (no padding), so a single memcpy works; we still
        // honour w_stride to be safe on other resolutions.
        const uint32_t row_bytes = input_width_ * 3;
        const uint32_t w_stride = input_attr_.w_stride > 0 ? input_attr_.w_stride : row_bytes;
        uint8_t* dst = static_cast<uint8_t*>(input_mem_->virt_addr);
        if (w_stride == row_bytes) {
            memcpy(dst, frame_data, static_cast<size_t>(row_bytes) * input_height_);
        } else {
            for (uint32_t y = 0; y < input_height_; ++y) {
                memcpy(dst + static_cast<size_t>(y) * w_stride,
                       frame_data + static_cast<size_t>(y) * row_bytes,
                       row_bytes);
            }
        }

        // Run inference. NPU writes output directly into output_mem_->virt_addr,
        // already converted to float32 (we set output_attr_.type=FLOAT32 at load).
        int ret = rknn_run(ctx_, nullptr);
        if (ret < 0) {
            std::cerr << "[RknnBackend] rknn_run failed: " << ret << "\n";
            return dets;
        }

        // DIAGNOSTIC: dump first/last values + max to understand output content.
        // Also probe both possible layouts: [1,84,8400] (rows-major) vs
        // [1,8400,84] (anchors-major) — the YOLOv8 RKNN export is ambiguous.
        {
            const float* dbg = static_cast<const float*>(output_mem_->virt_addr);
            const size_t total = static_cast<size_t>(output_rows_) * output_anchors_;
            float mx = -1e30f, mn = 1e30f;
            for (size_t i = 0; i < total; ++i) {
                if (dbg[i] > mx) mx = dbg[i];
                if (dbg[i] < mn) mn = dbg[i];
            }
            std::cerr << "[RknnBackend] out diag: n=" << total
                      << " first5=[" << dbg[0] << "," << dbg[1] << "," << dbg[2]
                      << "," << dbg[3] << "," << dbg[4] << "]"
                      << " min=" << mn << " max=" << mx << "\n";
            // rows-major [1,84,8400]: score-class0-anchor0 = dbg[4*8400+0]
            std::cerr << "[RknnBackend]   rows-major: score[c0,a0]=dbg[4*8400+0]="
                      << dbg[4 * output_anchors_ + 0]
                      << " score[c0,a100]=" << dbg[4 * output_anchors_ + 100] << "\n";
            // anchors-major [1,8400,84]: score-class0-anchor0 = dbg[0*84+4]
            std::cerr << "[RknnBackend]   anchors-major: score[a0,c0]=dbg[0*84+4]="
                      << dbg[0 * output_rows_ + 4]
                      << " score[a100,c0]=" << dbg[100 * output_rows_ + 4] << "\n";
        }

        // Parse YOLOv8 output [1, 4+nc, anchors] (float32, row-major).
        // Same numeric logic as crates/yolov8/src/lib.rs (postprocess).
        if (output_rows_ < 5 || output_anchors_ == 0) {
            std::cerr << "[RknnBackend] output shape not queried; cannot parse\n";
            return dets;
        }

        const uint32_t rows = output_rows_;
        const uint32_t anchors = output_anchors_;
        const uint32_t nc = rows - 4;

        // Output buffer is float32 already (output_attr_.type=FLOAT32 forced).
        const float* out = static_cast<const float*>(output_mem_->virt_addr);
        std::vector<float> floats(out, out + static_cast<size_t>(rows) * anchors);

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
        const float* f = floats.data();
        for (uint32_t a = 0; a < anchors; ++a) {
            const float cx = f[0 * anchors + a];
            const float cy = f[1 * anchors + a];
            const float w = f[2 * anchors + a];
            const float h = f[3 * anchors + a];

            uint32_t best_id = 0;
            float best_score = -INFINITY;
            for (uint32_t c = 0; c < nc; ++c) {
                const float s = f[(4 + c) * anchors + a];
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

    // Zero-copy IO state (allocated once at load, reused per frame).
    rknn_tensor_attr input_attr_;       // forced to UINT8 / NHWC
    rknn_tensor_attr output_attr_;      // forced to FLOAT32 (NPU converts native fp16/int8)
    rknn_tensor_mem* input_mem_ = nullptr;
    rknn_tensor_mem* output_mem_ = nullptr;
    bool is_quant_ = false;             // int8 model (vs float16) — diagnostics
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
