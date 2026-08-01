// bridge_main.h — declaration of run_bridge() for main.cpp.

#pragma once

#include <string>

namespace rknn_bridge {

// Run the bridge main loop. Returns process exit code.
int run_bridge(const std::string& socket_path);

}  // namespace rknn_bridge
