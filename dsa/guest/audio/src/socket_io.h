// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// Blocking reads and writes of the audio port's frames on a socket: each frame goes after
// its length as a little-endian 32-bit number. Used for the handshake of every stream and for
// the sound an input stream receives.

#pragma once

#include <cstddef>
#include <cstdint>
#include <vector>

namespace dsa::audio {

// The largest frame the host may send the HAL.
constexpr uint32_t kMaxHostFrame = 64 * 1024;

// Writes all of `bytes`, within the socket's send timeout. False when the socket failed.
bool writeAll(int fd, const std::vector<uint8_t>& bytes);

// Reads exactly `size` bytes, within the socket's receive timeout. False when the socket
// failed or closed.
bool readAll(int fd, uint8_t* out, size_t size);

// Reads one frame from the host. False when the socket failed or closed, or the frame is
// larger than kMaxHostFrame.
bool readFrame(int fd, std::vector<uint8_t>* frame);

}  // namespace dsa::audio
