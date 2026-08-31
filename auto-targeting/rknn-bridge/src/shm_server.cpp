// shm_server.cpp — Unix socket server with shared memory support.

#include "shm_server.h"

#include <arpa/inet.h>  // htonl/ntohl — canonical big-endian length prefix
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
        // Accept a connection (blocking; the main loop is single-threaded).
        client_fd_ = accept(server_fd_, nullptr, nullptr);
        if (client_fd_ < 0) {
            return "";
        }
        // KNOWN_ISSUES #12: не виснуть вечно на застрявшем клиенте —
        // 30-секундный таймаут на чтение (максимальный разумный инференс
        // плюс запас).
        struct timeval tv {};
        tv.tv_sec = 30;
        setsockopt(client_fd_, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    }

    // Read 4-byte length prefix.
    //
    // The wire format is CANONICAL BIG-ENDIAN (network byte order) — the Rust
    // client writes `len.to_be_bytes()` (bridge_client.rs). We previously read
    // the prefix as native uint32_t, which is little-endian on x86/aarch64 and
    // therefore incompatible with the Rust side (the bridge would mis-read
    // every frame length on any little-endian target, i.e. all our hardware).
    // ntohl() converts the wire big-endian value to host order.
    uint32_t length_be = 0;
    ssize_t n = read(client_fd_, &length_be, 4);
    if (n != 4) {
        // KNOWN_ISSUES #12: disconnect/timeout — закрываем и пере-accept
        // следующего клиента (раньше мёртвый fd висел и read падал вечно
        // в hot-loop без надежды на нового клиента).
        close(client_fd_);
        client_fd_ = -1;
        return "";
    }
    const uint32_t length = ntohl(length_be);
    // Sanitize: повреждённый length-prefix не должен аллоцировать гигабайты.
    if (length == 0 || length > 64u * 1024 * 1024) {
        close(client_fd_);
        client_fd_ = -1;
        return "";
    }

    // Read the message
    std::string message(length, '\0');
    size_t total = 0;
    while (total < length) {
        n = read(client_fd_, &message[total], length - total);
        if (n <= 0) {
            close(client_fd_);
            client_fd_ = -1;
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

    // 4-byte length prefix (canonical big-endian, matches Rust's to_be_bytes)
    // + JSON payload.
    const uint32_t length = static_cast<uint32_t>(json.size());
    const uint32_t length_be = htonl(length);
    if (write(client_fd_, &length_be, 4) != 4) {
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
