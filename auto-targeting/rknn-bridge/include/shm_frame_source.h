// shm_frame_source.h — named shared-memory frame source for the bridge (M6).
//
// The Rust client creates a named tmpfs segment (/dev/shm/<name>) holding
// N fixed-size frame buffers (double buffering). The init handshake passes
// the segment name; each infer request names the buffer index. The bridge
// mmaps the segment once and reads frames in place — no base64, no copy
// on the Rust side beyond the producer's own write.
//
// Wire contract (D-016):
//   init:  {"type":"init", ..., "frame_shm":"/dev/shm/at-infer",
//           "frame_shm_buffers":2, "frame_shm_size":1228800}
//   infer: {"type":"infer", "frame_seq":N, "frame_shm_buf":0|1, ...}
// Missing frame_shm in infer → the client fell back to base64 (compat).

#pragma once

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

namespace rknn_bridge {

class ShmFrameSource {
public:
    ShmFrameSource() = default;
    ~ShmFrameSource();

    ShmFrameSource(const ShmFrameSource&) = delete;
    ShmFrameSource& operator=(const ShmFrameSource&) = delete;

    // Open + mmap an existing segment created by the client.
    // `buffer_count` frames of `frame_size` bytes each are expected.
    // Returns false and fills `error` on failure; safe to retry.
    bool open(const std::string& path, uint32_t buffer_count,
              uint64_t frame_size, std::string& error);

    bool is_open() const { return mapping_ != nullptr; }

    // Read one frame buffer by index into `out` (out is resized).
    // `expected_bytes` comes from init (input_width*input_height*3).
    bool read_frame(uint32_t buffer_index, uint64_t expected_bytes,
                    std::vector<uint8_t>& out, std::string& error) const;

    const std::string& path() const { return path_; }

private:
    void close();

    std::string path_;
    void* mapping_ = nullptr;   // mmap result
    size_t map_len_ = 0;        // total mapped bytes
    uint32_t buffer_count_ = 0;
    uint64_t frame_size_ = 0;   // per-buffer bytes
    int fd_ = -1;
};

}  // namespace rknn_bridge
