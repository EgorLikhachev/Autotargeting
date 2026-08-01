// shm_server.h — Unix domain socket server with shared memory support.
//
// Listens on a Unix socket, receives control messages (JSON), and receives
// frame data via memfd shared memory (file descriptors passed via SCM_RIGHTS).

#pragma once

#include <functional>
#include <memory>
#include <string>

namespace rknn_bridge {

// A received frame: metadata from the JSON envelope + raw pixel data
// (copied from the shared memory region).
struct ReceivedFrame {
    uint64_t frame_seq;
    std::vector<uint8_t> data;  // raw pixel data
    uint64_t captured_at_ms;
};

// Callback type for handling incoming frames.
using FrameCallback = std::function<void(const ReceivedFrame&)>;

// Unix socket server that accepts connections from the Rust orchestrator.
class ShmServer {
public:
    ShmServer(const std::string& socket_path);
    ~ShmServer();

    // Start listening on the socket. Returns true on success.
    bool start(std::string& error);

    // Stop the server and close the socket.
    void stop();

    // Receive a single message (blocking).
    // Returns the JSON string of the message, or empty on error/disconnect.
    std::string receive_message();

    // Receive a frame: reads the InferRequest JSON, then receives the
    // shared memory fd via SCM_RIGHTS and copies the data.
    // Returns true on success, false on disconnect/error.
    bool receive_frame(ReceivedFrame& frame, std::string& error);

    // Send a JSON response back to the client.
    bool send_response(const std::string& json);

    // Check if the server is running.
    bool is_running() const;

private:
    std::string socket_path_;
    int server_fd_;
    int client_fd_;
    bool running_;
};

}  // namespace rknn_bridge
