// bridge_main.cpp — main entry point for the rknn-bridge microservice.
//
// Handles the IPC protocol: receives init/infer/health/shutdown messages
// from the Rust orchestrator, dispatches to the InferenceBackend, and
// sends responses.
//
// JSON serialization is minimal (hand-rolled) to avoid adding a JSON library
// dependency. If we need more complex messages, switch to nlohmann/json.

#include "protocol.h"
#include "rknn_model.h"
#include "shm_server.h"
#include "shm_frame_source.h"

#include <chrono>
#include <csignal>
#include <cstdlib>
#include <iostream>
#include <string>
#include <thread>

#include <unistd.h>

namespace rknn_bridge {

static volatile bool g_should_stop = false;

static void signal_handler(int sig) {
    (void)sig;
    g_should_stop = true;
}

// === Minimal JSON helpers ===
// We hand-roll the JSON we need to avoid a dependency. This is fragile but
// sufficient for our small protocol.

std::string escape_json_string(const std::string& s) {
    std::string out;
    out.reserve(s.size() + 2);
    for (char c : s) {
        switch (c) {
            case '"':  out += "\\\""; break;
            case '\\': out += "\\\\"; break;
            case '\n': out += "\\n"; break;
            case '\r': out += "\\r"; break;
            case '\t': out += "\\t"; break;
            default:   out += c; break;
        }
    }
    return out;
}

std::string serialize_init_request(const InitRequest& req) {
    std::string s;
    s += "{\"type\":\"init\",\"model_path\":\"" + escape_json_string(req.model_path) + "\"";
    s += ",\"input_width\":" + std::to_string(req.input_width);
    s += ",\"input_height\":" + std::to_string(req.input_height);
    s += ",\"input_format\":\"" + escape_json_string(req.input_format) + "\"";
    s += ",\"confidence_threshold\":" + std::to_string(req.confidence_threshold);
    s += ",\"nms_threshold\":" + std::to_string(req.nms_threshold);
    s += "}";
    return s;
}

bool parse_init_request(const std::string& json, InitRequest& req) {
    // Very simple parser — find "key":value pairs.
    // For production, use a real JSON library.
    auto find_string = [&](const std::string& key, std::string& out) -> bool {
        std::string needle = "\"" + key + "\":\"";
        size_t pos = json.find(needle);
        if (pos == std::string::npos) return false;
        pos += needle.size();
        size_t end = json.find('"', pos);
        if (end == std::string::npos) return false;
        out = json.substr(pos, end - pos);
        return true;
    };
    auto find_number = [&](const std::string& key, double& out) -> bool {
        std::string needle = "\"" + key + "\":";
        size_t pos = json.find(needle);
        if (pos == std::string::npos) return false;
        pos += needle.size();
        try {
            out = std::stod(json.substr(pos));
        } catch (...) {
            return false;
        }
        return true;
    };
    auto find_float = [&](const std::string& key, float& out) -> bool {
        double d;
        if (!find_number(key, d)) return false;
        out = static_cast<float>(d);
        return true;
    };
    auto find_uint = [&](const std::string& key, uint32_t& out) -> bool {
        double d;
        if (!find_number(key, d)) return false;
        out = static_cast<uint32_t>(d);
        return true;
    };

    if (!find_string("model_path", req.model_path)) return false;
    if (!find_uint("input_width", req.input_width)) return false;
    if (!find_uint("input_height", req.input_height)) return false;
    if (!find_string("input_format", req.input_format)) return false;
    if (!find_float("confidence_threshold", req.confidence_threshold)) return false;
    if (!find_float("nms_threshold", req.nms_threshold)) return false;
    // M6: опциональный SHM-путь кадра (отсутствует → base64 fallback).
    find_string("frame_shm", req.frame_shm);        // может отсутствовать
    req.frame_shm_buffers = 0;
    req.frame_shm_size = 0;
    {
        double d;
        if (find_number("frame_shm_buffers", d)) req.frame_shm_buffers = static_cast<uint32_t>(d);
        if (find_number("frame_shm_size", d)) req.frame_shm_size = static_cast<uint64_t>(d);
    }
    return true;
}

std::string serialize_infer_response(const InferResponse& resp) {
    std::string s;
    s += "{\"type\":\"infer_ack\",\"ok\":" + std::string(resp.ok ? "true" : "false");
    if (!resp.ok) {
        s += ",\"error\":\"" + escape_json_string(resp.error) + "\"";
    } else {
        s += ",\"frame_seq\":" + std::to_string(resp.frame_seq);
        s += ",\"latency_ms\":" + std::to_string(resp.latency_ms);
        s += ",\"detections\":[";
        for (size_t i = 0; i < resp.detections.size(); ++i) {
            const auto& d = resp.detections[i];
            if (i > 0) s += ",";
            s += "{\"bbox\":{\"x\":" + std::to_string(d.bbox.x);
            s += ",\"y\":" + std::to_string(d.bbox.y);
            s += ",\"width\":" + std::to_string(d.bbox.width);
            s += ",\"height\":" + std::to_string(d.bbox.height);
            s += "},\"class\":\"" + escape_json_string(d.class_name) + "\"";
            s += ",\"class_id\":" + std::to_string(d.class_id);
            s += ",\"confidence\":" + std::to_string(d.confidence);
            s += "}";
        }
        s += "]";
    }
    s += "}";
    return s;
}

bool parse_infer_request(const std::string& json, InferRequest& req) {
    auto find_uint64 = [&](const std::string& key, uint64_t& out) -> bool {
        std::string needle = "\"" + key + "\":";
        size_t pos = json.find(needle);
        if (pos == std::string::npos) return false;
        pos += needle.size();
        try {
            out = std::stoull(json.substr(pos));
        } catch (...) {
            return false;
        }
        return true;
    };

    if (!find_uint64("frame_seq", req.frame_seq)) return false;
    if (!find_uint64("captured_at_ms", req.captured_at_ms)) return false;
    req.shm_fd = -1;
    req.shm_size = 0;
    req.shm_buf = 0;  // M6: индекс буфера в именованном сегменте
    {
        uint64_t b;
        if (find_uint64("frame_shm_buf", b)) req.shm_buf = static_cast<uint32_t>(b);
    }
    return true;
}

// === base64 decoder ===
// Decodes a standard base64 string into raw bytes. Used to extract the inline
// frame data sent by the Rust client in the `frame_data_b64` JSON field (the
// SHM/SCM_RIGHTS path is still a TODO — see SDD §15 #2 — so for now frames
// travel inline as base64). Mirrors the Rust hand-rolled base64 in
// bridge_client.rs.
static inline int b64_char_value(char c) {
    if (c >= 'A' && c <= 'Z') return c - 'A';
    if (c >= 'a' && c <= 'z') return c - 'a' + 26;
    if (c >= '0' && c <= '9') return c - '0' + 52;
    if (c == '+') return 62;
    if (c == '/') return 63;
    return -1;
}

static std::vector<uint8_t> base64_decode(const std::string& s) {
    std::vector<uint8_t> out;
    out.reserve((s.size() / 4) * 3);
    int val = 0, valb = -8;
    for (char c : s) {
        if (c == '=' || c == '\n' || c == '\r' || c == ' ') continue;
        int d = b64_char_value(c);
        if (d < 0) continue;
        val = (val << 6) | d;
        valb += 6;
        if (valb >= 0) {
            out.push_back(static_cast<uint8_t>((val >> valb) & 0xFF));
            valb -= 8;
        }
    }
    return out;
}

// Extract the `frame_data_b64` string field from the JSON envelope.
// Hand-rolled substring extraction (consistent with the rest of this file's
// minimal JSON handling). Returns empty string if absent.
static std::string extract_frame_data_b64(const std::string& json) {
    const std::string key = "\"frame_data_b64\":\"";
    size_t start = json.find(key);
    if (start == std::string::npos) return "";
    start += key.size();
    size_t end = json.find("\"", start);
    if (end == std::string::npos) return "";
    return json.substr(start, end - start);
}

std::string serialize_health_response(const HealthResponse& resp) {
    std::string s;
    s += "{\"type\":\"health_ack\",\"ok\":" + std::string(resp.ok ? "true" : "false");
    s += ",\"model_loaded\":" + std::string(resp.model_loaded ? "true" : "false");
    s += ",\"npu_utilization\":" + std::to_string(resp.npu_utilization);
    s += ",\"backend\":\"" + escape_json_string(resp.backend) + "\"";
    s += "}";
    return s;
}

// === Main loop ===

int run_bridge(const std::string& socket_path) {
    // Register signal handlers
    signal(SIGINT, signal_handler);
    signal(SIGTERM, signal_handler);

    // Create the inference backend (RKNN if available, else stub)
    auto backend = create_backend();
    std::cerr << "[bridge] Using backend: " << backend->backend_name() << "\n";

    // Start the server
    ShmServer server(socket_path);
    std::string error;
    if (!server.start(error)) {
        std::cerr << "[bridge] Failed to start server: " << error << "\n";
        return 1;
    }

    std::cerr << "[bridge] Ready. Waiting for init message...\n";

    // M6: named SHM frame segment (opened on init if the client requests it).
    ShmFrameSource frame_source;
    InitRequest init_req{};  // zero-init: no shm path until init arrives

    // Main loop
    while (!g_should_stop && server.is_running()) {
        std::string msg = server.receive_message();
        if (msg.empty()) {
            if (g_should_stop) break;
            // Client disconnected or error — wait for a new connection
            continue;
        }

        // Dispatch based on message type
        if (msg.find("\"type\":\"init\"") != std::string::npos) {
            InitRequest req;
            if (!parse_init_request(msg, req)) {
                std::string resp = "{\"type\":\"init_ack\",\"ok\":false,\"error\":\"parse failed\"}";
                server.send_response(resp);
                continue;
            }

            std::string init_error;
            bool ok = backend->load_model(
                req.model_path, req.input_width, req.input_height,
                req.input_format, req.confidence_threshold, req.nms_threshold,
                init_error);

            if (ok && !req.frame_shm.empty()) {
                std::string serr;
                if (!frame_source.open(req.frame_shm, req.frame_shm_buffers,
                                       req.frame_shm_size, serr)) {
                    std::cerr << "[bridge] frame shm open failed: " << serr
                              << " — falling back to base64" << std::endl;
                } else {
                    std::cerr << "[bridge] frame shm: " << req.frame_shm
                              << " (" << req.frame_shm_buffers << " x "
                              << req.frame_shm_size << " B)" << std::endl;
                }
            }
            if (ok) {
                init_req = req;  // запоминаем для расчёта размера кадра
            }

            InitResponse resp;
            resp.ok = ok;
            resp.error = init_error;
            resp.output_classes = backend->output_classes();
            resp.backend = backend->backend_name();

            std::string resp_json = "{\"type\":\"init_ack\",\"ok\":" + std::string(ok ? "true" : "false");
            if (!ok) {
                resp_json += ",\"error\":\"" + escape_json_string(init_error) + "\"";
            } else {
                resp_json += ",\"output_classes\":" + std::to_string(resp.output_classes);
                resp_json += ",\"backend\":\"" + escape_json_string(resp.backend) + "\"";
            }
            resp_json += "}";
            server.send_response(resp_json);

        } else if (msg.find("\"type\":\"infer\"") != std::string::npos) {
            InferRequest req;
            if (!parse_infer_request(msg, req)) {
                std::string resp = "{\"type\":\"infer_ack\",\"ok\":false,\"error\":\"parse failed\"}";
                server.send_response(resp);
                continue;
            }

            // M6 (D-016): кадр из именованного SHM-сегмента, если он был
            // объявлен в init; иначе — старый base64-путь (fallback).
            std::vector<uint8_t> frame_data;
            bool have_frame = false;
            if (frame_source.is_open()) {
                // Авторитетный размер кадра — то, что клиент объявил в init
                // (letterboxed тензор, напр. 640x640x3).
                const uint64_t expect = init_req.frame_shm_size;
                std::string ferr;
                have_frame = frame_source.read_frame(req.shm_buf, expect,
                                                     frame_data, ferr);
                if (!have_frame) {
                    std::cerr << "[bridge] shm frame read failed: " << ferr
                              << std::endl;
                }
            }
            if (!have_frame) {
                std::string b64 = extract_frame_data_b64(msg);
                frame_data = base64_decode(b64);
            }

            auto start = std::chrono::high_resolution_clock::now();
            auto dets = backend->infer(frame_data.data(), frame_data.size(), req.frame_seq);
            auto end = std::chrono::high_resolution_clock::now();
            uint32_t latency_ms = std::chrono::duration_cast<std::chrono::milliseconds>(end - start).count();

            InferResponse resp;
            resp.ok = true;
            resp.frame_seq = req.frame_seq;
            resp.latency_ms = latency_ms;
            resp.detections = dets;

            server.send_response(serialize_infer_response(resp));

        } else if (msg.find("\"type\":\"health\"") != std::string::npos) {
            HealthResponse resp;
            resp.ok = true;
            resp.model_loaded = backend->is_loaded();
            resp.npu_utilization = backend->npu_utilization();
            resp.backend = backend->backend_name();
            server.send_response(serialize_health_response(resp));

        } else if (msg.find("\"type\":\"shutdown\"") != std::string::npos) {
            std::cerr << "[bridge] Received shutdown — exiting\n";
            server.send_response("{\"type\":\"shutdown_ack\"}");
            break;

        } else {
            std::cerr << "[bridge] Unknown message: " << msg << "\n";
        }
    }

    server.stop();
    std::cerr << "[bridge] Shutdown complete\n";
    return 0;
}

}  // namespace rknn_bridge
