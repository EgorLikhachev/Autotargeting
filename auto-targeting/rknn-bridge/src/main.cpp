// main.cpp — entry point for the rknn-bridge binary.
//
// Usage:
//   rknn-bridge --socket /tmp/rknn-bridge.sock --model /path/to/model.rknn
//
// The bridge starts, loads the model, and listens for requests from the
// Rust orchestrator (cv-inference crate) over the Unix socket.

#include "bridge_main.h"

#include <iostream>
#include <string>

#include <getopt.h>

static void print_usage(const char* prog) {
    std::cerr << "Usage: " << prog << " [options]\n";
    std::cerr << "Options:\n";
    std::cerr << "  -s, --socket PATH   Unix socket path (default: /tmp/rknn-bridge.sock)\n";
    std::cerr << "  -m, --model PATH    Path to .rknn model file (loaded on init message)\n";
    std::cerr << "  -h, --help          Show this help\n";
}

int main(int argc, char* argv[]) {
    std::string socket_path = "/tmp/rknn-bridge.sock";
    std::string model_path;

    static struct option long_opts[] = {
        {"socket", required_argument, 0, 's'},
        {"model",  required_argument, 0, 'm'},
        {"help",   no_argument,       0, 'h'},
        {0, 0, 0, 0}
    };

    int opt;
    while ((opt = getopt_long(argc, argv, "s:m:h", long_opts, nullptr)) != -1) {
        switch (opt) {
            case 's':
                socket_path = optarg;
                break;
            case 'm':
                model_path = optarg;
                break;
            case 'h':
                print_usage(argv[0]);
                return 0;
            default:
                print_usage(argv[0]);
                return 1;
        }
    }

    std::cerr << "=== rknn-bridge ===\n";
    std::cerr << "Socket: " << socket_path << "\n";
    if (!model_path.empty()) {
        std::cerr << "Model:  " << model_path << "\n";
    }
    std::cerr << "\n";

    return rknn_bridge::run_bridge(socket_path);
}
