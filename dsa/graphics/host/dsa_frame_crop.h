// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// A task's frame inside its ColorBuffer. The guest's hwcomposer makes each task's
// ColorBuffers at the display's size, so a resize remakes none of them, and composes the
// task's frame into their top-left corner; the task display's size says how much of the
// buffer the frame fills. Vulkan reads a ColorBuffer back only whole, so the pixel
// presentation reads the whole buffer and keeps the frame's rows from it.

#ifndef DSA_FRAME_CROP_H
#define DSA_FRAME_CROP_H

#include <cstddef>
#include <cstdint>
#include <cstring>

namespace dsa {

struct FrameSize {
    uint32_t width = 0;
    uint32_t height = 0;
};

// The part of a `buffer_width` x `buffer_height` ColorBuffer a frame of
// `frame_width` x `frame_height` fills: the whole buffer when the frame has no size of its
// own, and never more than the buffer.
inline FrameSize frame_in_buffer(uint32_t buffer_width, uint32_t buffer_height,
                                 uint32_t frame_width, uint32_t frame_height) {
    FrameSize size;
    size.width = frame_width == 0 || frame_width > buffer_width ? buffer_width : frame_width;
    size.height =
        frame_height == 0 || frame_height > buffer_height ? buffer_height : frame_height;
    return size;
}

// Moves the top-left `width` x `height` RGBA pixels of a picture `buffer_width` pixels wide
// to the start of `pixels`, row after row, so they read as a picture `width` pixels wide.
inline void keep_top_left(uint8_t* pixels, uint32_t buffer_width, uint32_t width,
                          uint32_t height) {
    if (width >= buffer_width) {
        return;
    }
    const std::size_t row = std::size_t(width) * 4;
    const std::size_t stride = std::size_t(buffer_width) * 4;
    // Each row moves to a place no later than its own, so going down the rows never
    // overwrites a row not yet moved.
    for (uint32_t y = 1; y < height; y++) {
        std::memmove(pixels + y * row, pixels + y * stride, row);
    }
}

}  // namespace dsa

#endif  // DSA_FRAME_CROP_H
