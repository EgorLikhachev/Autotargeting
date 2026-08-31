// shm_frame_source.cpp — named /dev/shm frame segment reader (M6, D-016).

#include "shm_frame_source.h"

#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#include <cerrno>
#include <cstring>

namespace rknn_bridge {

ShmFrameSource::~ShmFrameSource() { close(); }

void ShmFrameSource::close() {
    if (mapping_ != nullptr && mapping_ != MAP_FAILED) {
        munmap(mapping_, map_len_);
    }
    mapping_ = nullptr;
    map_len_ = 0;
    if (fd_ >= 0) {
        ::close(fd_);
        fd_ = -1;
    }
}

bool ShmFrameSource::open(const std::string& path, uint32_t buffer_count,
                          uint64_t frame_size, std::string& error) {
    if (buffer_count == 0 || frame_size == 0) {
        error = "shm frame source: zero buffer_count/frame_size";
        return false;
    }
    close();

    fd_ = ::open(path.c_str(), O_RDWR | O_CLOEXEC);
    if (fd_ < 0) {
        error = "open(" + path + "): " + std::string(std::strerror(errno));
        return false;
    }

    struct stat st {};
    if (fstat(fd_, &st) != 0) {
        error = "fstat: " + std::string(std::strerror(errno));
        close();
        return false;
    }
    const uint64_t needed =
        static_cast<uint64_t>(buffer_count) * frame_size;
    if (static_cast<uint64_t>(st.st_size) < needed) {
        error = "segment too small: " + std::to_string(st.st_size) +
                " < " + std::to_string(needed) + " (" +
                std::to_string(buffer_count) + "x" +
                std::to_string(frame_size) + ")";
        close();
        return false;
    }

    map_len_ = static_cast<size_t>(st.st_size);
    mapping_ = mmap(nullptr, map_len_, PROT_READ, MAP_SHARED, fd_, 0);
    if (mapping_ == MAP_FAILED) {
        error = "mmap: " + std::string(std::strerror(errno));
        mapping_ = nullptr;
        close();
        return false;
    }

    path_ = path;
    buffer_count_ = buffer_count;
    frame_size_ = frame_size;
    return true;
}

bool ShmFrameSource::read_frame(uint32_t buffer_index, uint64_t expected_bytes,
                                std::vector<uint8_t>& out,
                                std::string& error) const {
    if (mapping_ == nullptr) {
        error = "shm frame source not open";
        return false;
    }
    if (buffer_index >= buffer_count_) {
        error = "buffer index " + std::to_string(buffer_index) +
                " out of range (" + std::to_string(buffer_count_) + ")";
        return false;
    }
    if (expected_bytes > frame_size_) {
        // Tolerable if the producer sized buffers for the largest frame;
        // otherwise the request is inconsistent with init.
        error = "expected " + std::to_string(expected_bytes) +
                " > buffer size " + std::to_string(frame_size_);
        return false;
    }
    const auto* base = static_cast<const uint8_t*>(mapping_) +
                       static_cast<size_t>(buffer_index) * frame_size_;
    out.assign(base, base + expected_bytes);
    return true;
}

}  // namespace rknn_bridge
