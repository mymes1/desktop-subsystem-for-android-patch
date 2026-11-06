// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The camera port's messages, through the codec generated from dsa/protocol/control.proto.
// Each frame is one encoded Envelope; on the socket each goes with its length in front.

#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <variant>
#include <vector>

namespace dsa::camera {

// Where a camera faces, as the protocol numbers it.
enum class Facing : int32_t { Unspecified = 0, Front = 1, Back = 2, External = 3 };

// One picture size a camera delivers, and its frame rates.
struct Mode {
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t maxFps = 0;
    uint32_t minFps = 0;
};

// One camera of the host.
struct HostCamera {
    std::string id;
    std::string name;
    Facing facing = Facing::External;
    uint32_t sensorOrientation = 0;
    std::vector<Mode> modes;
};

// What the host sends on the camera port.
struct Accepted {};
struct Rejected {
    std::string detail;
};
struct CameraList {
    std::vector<HostCamera> cameras;
};
struct Opened {
    bool ok = false;
    std::string error;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t fps = 0;
};
struct Picture {
    uint64_t sequence = 0;
    uint64_t timestampNs = 0;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t stride = 0;
    std::string nv12;
};
struct StillTaken {
    uint32_t requestId = 0;
    std::string jpeg;
    std::string error;
};
struct CameraFailed {
    std::string error;
};

using HostMessage = std::variant<std::monostate, Accepted, Rejected, CameraList, Opened, Picture, StillTaken,
                                 CameraFailed>;

// What a frame from the host says; std::monostate for one that cannot be decoded or that the
// camera port does not carry.
HostMessage decodeHostFrame(const uint8_t* data, size_t size);

// The camera HAL's Hello on the camera port.
std::vector<uint8_t> encodeHello(const std::string& build);
std::vector<uint8_t> encodeOpen(const std::string& id, uint32_t width, uint32_t height, uint32_t fps);
std::vector<uint8_t> encodeStop();
std::vector<uint8_t> encodeStill(uint32_t requestId, uint32_t width, uint32_t height, uint32_t quality);

// `frame` with its length in front, as the socket carries it.
std::vector<uint8_t> withLength(const std::vector<uint8_t>& frame);

}  // namespace dsa::camera
