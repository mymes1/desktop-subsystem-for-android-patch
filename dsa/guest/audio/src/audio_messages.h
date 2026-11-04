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
// An input stream's format: 48 kHz, stereo, 16-bit, read ramesPerBurst frames at a time.
std::vector<uint8_t> encodeInputOpen(const std::string& name, uint32_t framesPerBurst);

struct Accepted {};
struct Rejected {
    std::string detail;
};
struct Ready {
    uint32_t bufferedFrames = 0;
    uint32_t deviceFrames = 0;
};
// The host's answer to an input stream's open. An empty error means the capture device records.
struct InputOpened {
    std::string error;
    uint32_t deviceFrames = 0;
};
// Sound the host recorded, position frames into the input stream.
struct Pcm {
    uint64_t position = 0;
    std::string data;
};
using HostMessage = std::variant<std::monostate, Accepted, Rejected, Ready, InputOpened, Pcm>;

// What a frame from the host means to the HAL; std::monostate for anything else.
HostMessage decodeHostFrame(const uint8_t* data, size_t size);

// Prepends the frame's length, as the socket carries it.
std::vector<uint8_t> withLength(const std::vector<uint8_t>& frame);

}  // namespace dsa::audio
