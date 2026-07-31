cmake_minimum_required(VERSION 3.16)
project(rknn-bridge LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# Build type
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release)
endif()

# Compiler flags
add_compile_options(-Wall -Wextra -Wpedantic)
if(CMAKE_BUILD_TYPE STREQUAL "Release")
    add_compile_options(-O2)
endif()

# RKNN SDK path — set this to where you extracted the RKNPU2 SDK.
# Download from: https://github.com/airockchip/rknn-toolkit2
# Default: assume it's at /opt/rknn-toolkit2
set(RKNN_SDK_PATH "/opt/rknn-toolkit2" CACHE PATH "Path to RKNPU2 SDK")
set(RKNN_LIB_PATH "${RKNN_SDK_PATH}/runtime/Linux/librknn_api/aarch64" CACHE PATH "Path to librknnrt.so")

# Find librknnrt (optional — we build a stub if it's not available)
find_library(RKNN_LIBRARY
    NAMES rknnrt
    PATHS ${RKNN_LIB_PATH}
    NO_DEFAULT_PATH
)

if(RKNN_LIBRARY)
    message(STATUS "Found RKNN library: ${RKNN_LIBRARY}")
    set(HAVE_RKNN TRUE)
    add_compile_definitions(HAVE_RKNN=1)
else()
    message(WARNING
        "RKNN library not found at ${RKNN_LIB_PATH} — building stub backend.\n"
        "The stub returns fake detections for testing without an NPU.\n"
        "To enable real NPU inference:\n"
        "  1. Download RKNPU2 SDK: https://github.com/airockchip/rknn-toolkit2\n"
        "  2. Set RKNN_SDK_PATH: cmake -DRKNN_SDK_PATH=/path/to/sdk .."
    )
    set(HAVE_RKNN FALSE)
    add_compile_definitions(HAVE_RKNN=0)
endif()

# Find threads (for std::thread)
find_package(Threads REQUIRED)

# === rknn-bridge library ===
add_library(rknn_bridge STATIC
    src/rknn_model.cpp
    src/shm_server.cpp
    src/nms.cpp
    src/bridge_main.cpp
)
target_include_directories(rknn_bridge PUBLIC include)

if(HAVE_RKNN)
    target_link_libraries(rknn_bridge PUBLIC ${RKNN_LIBRARY})
endif()

target_link_libraries(rknn_bridge PUBLIC Threads::Threads)

# === rknn-bridge executable ===
add_executable(rknn-bridge src/main.cpp)
target_link_libraries(rknn-bridge PRIVATE rknn_bridge)

# Install
install(TARGETS rknn-bridge DESTINATION bin)
install(TARGETS rknn_bridge DESTINATION lib)
install(DIRECTORY include/ DESTINATION include/rknn_bridge)

# === Tests (optional) ===
option(BUILD_TESTS "Build unit tests" OFF)
if(BUILD_TESTS)
    enable_testing()
    add_executable(test_nms tests/test_nms.cpp)
    target_link_libraries(test_nms PRIVATE rknn_bridge)
    add_test(NAME nms COMMAND test_nms)
endif()
