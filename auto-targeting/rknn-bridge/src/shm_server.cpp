// shm_server.cpp — Unix socket server with shared memory support.

#include "shm_server.h"

#include <cstring>
#include <iostream>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace rknn_bridge {

ShmServer::ShmServer(const std::string& socket_path)
    : socket_path_(socket_path), server_fd_(-1), client_fd_(-1), running_(false) {}

ShmServer::~ShmServer() {
    stop();
}

bool ShmServer::start(std::string& error) {
    server_fd_ = socket(AF_UNIX, SOCK_STREAM, 0);
    if (server_fd_ < 0) {
        error = "socket() failed: " + std::string(strerror(errno));
        return false;
    }

    // Remove stale socket file
    unlink(socket_path_.c_str());

    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    strncpy(addr.sun_path, socket_path_.c_str(), sizeof(addr.sun_path) - 1);

    if (bind(server_fd_, reinterpret_cast<struct sockaddr*>(&addr), sizeof(addr)) < 0) {
        error = "bind() failed: " + std::string(strerror(errno));
        close(server_fd_);
        server_fd_ = -1;
        return false;
    }

    if (listen(server_fd_, 1) < 0) {
        error = "listen() failed: " + std::string(strerror(errno));
        close(server_fd_);
        server_fd_ = -1;
        return false;
    }

    running_ = true;
    std::cerr << "[ShmServer] Listening on " << socket_path_ << "\n";
    return true;
}

void ShmServer::stop() {
    if (client_fd_ >= 0) {
        close(client_fd_);
        client_fd_ = -1;
    }
    if (server_fd_ >= 0) {
        close(server_fd_);
        server_fd_ = -1;
    }
    if (!socket_path_.empty()) {
        unlink(socket_path_.c_str());
    }
    running_ = false;
}

std::string ShmServer::receive_message() {
    if (client_fd_ < 0) {
        // Accept a connection
        client_fd_ = accept(server_fd_, nullptr, nullptr);
        if (client_fd_ < 0) {
            return "";
        }
    }

    // Read 4-byte length prefix
    uint32_t length = 0;
    ssize_t n = read(client_fd_, &length, 4);
    if (n != 4) {
        return "";
    }

    // Read the message
    std::string message(length, '\0');
    size_t total = 0;
    while (total < length) {
        n = read(client_fd_, &message[total], length - total);
        if (n <= 0) {
            return "";
        }
        total += n;
    }

    return message;
}

bool ShmServer::receive_frame(ReceivedFrame& frame, std::string& error) {
    // The frame data should arrive as ancillary data (SCM_RIGHTS) alongside
    // the InferRequest JSON message.
    //
    // For simplicity, this stub implementation expects the frame data to be
    // sent as a regular message after the JSON envelope. Real implementation
    // would use recvmsg() with msg_control for the fd.
    //
    // TODO: implement proper SCM_RIGHTS fd passing.

    (void)error;
    frame.frame_seq = 0;
    frame.data.clear();
    return false;
}

bool ShmServer::send_response(const std::string& json) {
    if (client_fd_ < 0) {
        return false;
    }

    // 4-byte length prefix + JSON
    uint32_t length = static_cast<uint32_t>(json.size());
    if (write(client_fd_, &length, 4) != 4) {
        return false;
    }
    if (write(client_fd_, json.data(), length) != static_cast<ssize_t>(length)) {
        return false;
    }
    return true;
}

bool ShmServer::is_running() const {
    return running_;
}

}  // namespace rknn_bridge
