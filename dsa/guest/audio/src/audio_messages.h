// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The audio port's messages, through the codec generated from dsa/protocol/control.proto.
// Each frame is one encoded Envelope; on the socket each goes after its length as a
// little-endian 32-bit number.

#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <variant>
#include <vector>

#include "output_stream_logic.h"

namespace dsa::audio {

// The HAL's Hello: this build's protocol version and schema hash, as the guest's audio HAL.
std::vector<uint8_t> encodeHello(const std::string& build);
// The stream's format: 48 kHz, stereo, 16-bit, `framesPerBurst` frames at a time.
std::vector<uint8_t> encodeOpen(const std::string& name, uint32_t framesPerBurst);
std::vector<uint8_t> encodePcm(uint64_t position, const uint8_t* data, size_t size);
std::vector<uint8_t> encodeControl(HostCommand command);

struct Accepted {};
struct Rejected {
    std::string detail;
};
struct Ready {
    uint32_t bufferedFrames = 0;
    uint32_t deviceFrames = 0;
};
using HostMessage = std::variant<std::monostate, Accepted, Rejected, Ready>;

// What a frame from the host means to the HAL; std::monostate for anything else.
HostMessage decodeHostFrame(const uint8_t* data, size_t size);

// Prepends the frame's length, as the socket carries it.
std::vector<uint8_t> withLength(const std::vector<uint8_t>& frame);

}  // namespace dsa::audio
