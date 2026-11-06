// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The app's camera buffers, which reach the HAL as graphics buffer handles, made into
// AHardwareBuffers the HAL can lock and write. The platform's libnativewindow imports a handle
// (AHardwareBuffer_createFromHandle, a vendor call the NDK's headers leave out); the buffer
// keeps its own copy of the handle's file descriptors.

#pragma once

#include <android/hardware_buffer.h>

#include <cstdint>
#include <string>
#include <vector>

#include "frame_copy.h"

namespace dsa::camera {

// A graphics buffer handle's parts, as the AIDL NativeHandle carries them.
struct HandleParts {
    std::vector<int> fds;
    std::vector<int32_t> ints;
};

// Imports a buffer of `width` by `height` in `format` (an AHARDWAREBUFFER_FORMAT_ value) used
// as `usage`; nullptr when the handle is not one. The caller releases it.
AHardwareBuffer* importBuffer(const HandleParts& handle, uint32_t width, uint32_t height, uint32_t format,
                              uint64_t usage);

// Fills a picture buffer from the host's picture, scaled to `width` by `height`. False, with
// the reason in `error`, when the buffer cannot be written.
bool fillPicture(AHardwareBuffer* buffer, const Nv12& picture, uint32_t width, uint32_t height, std::string* error);

// Puts `jpeg` in a still buffer of `size` bytes, with the trailer Android reads its length from.
bool fillStill(AHardwareBuffer* buffer, size_t size, const std::string& jpeg, std::string* error);

// Waits up to `timeoutMs` for the fence `fd` (a sync file) to signal, and closes it. A fence
// of -1 is already signalled.
bool waitFence(int fd, int timeoutMs);

}  // namespace dsa::camera
