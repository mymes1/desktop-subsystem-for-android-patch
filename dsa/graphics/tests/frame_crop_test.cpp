// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The pixels a task window receives when its frames are read back to memory: the frame in
// the top-left corner of a ColorBuffer the size of the display, not the whole buffer, and
// read as one whole buffer, since Vulkan reads back no part of one.

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <vector>

#include "dsa_frame_crop.h"

namespace {

int failures = 0;

void check(bool ok, const char* test, const char* what) {
    if (!ok) {
        std::fprintf(stderr, "%s: %s\n", test, what);
        failures++;
    }
}

void pr3_a_task_frame_is_the_top_left_part_of_its_buffer() {
    const char* test = __func__;
    const dsa::FrameSize size = dsa::frame_in_buffer(1920, 1080, 548, 911);
    check(size.width == 548 && size.height == 911, test, "the frame's own size");
}

void pr3_a_frame_without_a_size_fills_its_buffer() {
    const char* test = __func__;
    const dsa::FrameSize size = dsa::frame_in_buffer(1920, 1080, 0, 0);
    check(size.width == 1920 && size.height == 1080, test, "the whole buffer");
}

void pr3_a_frame_is_never_larger_than_its_buffer() {
    const char* test = __func__;
    const dsa::FrameSize size = dsa::frame_in_buffer(640, 480, 800, 600);
    check(size.width == 640 && size.height == 480, test, "cut to the buffer");
}

void pr3_the_frame_rows_are_kept_from_the_whole_buffer() {
    const char* test = __func__;
    // A 4x3 buffer whose pixels are numbered by place; the frame is its top-left 2x2.
    const uint32_t buffer_width = 4;
    const uint32_t buffer_height = 3;
    std::vector<uint8_t> pixels(buffer_width * buffer_height * 4);
    for (uint32_t y = 0; y < buffer_height; y++) {
        for (uint32_t x = 0; x < buffer_width; x++) {
            for (uint32_t c = 0; c < 4; c++) {
                pixels[(y * buffer_width + x) * 4 + c] = uint8_t(y * 16 + x * 4 + c);
            }
        }
    }
    dsa::keep_top_left(pixels.data(), buffer_width, 2, 2);
    bool same = true;
    for (uint32_t y = 0; y < 2; y++) {
        for (uint32_t x = 0; x < 2; x++) {
            for (uint32_t c = 0; c < 4; c++) {
                same = same && pixels[(y * 2 + x) * 4 + c] == uint8_t(y * 16 + x * 4 + c);
            }
        }
    }
    check(same, test, "the frame's pixels, two pixels to a row");
}

void pr3_a_frame_as_wide_as_its_buffer_is_left_as_it_is() {
    const char* test = __func__;
    std::vector<uint8_t> pixels = {1, 2, 3, 4, 5, 6, 7, 8};
    const std::vector<uint8_t> before = pixels;
    dsa::keep_top_left(pixels.data(), 1, 1, 2);
    check(pixels == before, test, "nothing moves");
}

}  // namespace

int main() {
    pr3_a_task_frame_is_the_top_left_part_of_its_buffer();
    pr3_a_frame_without_a_size_fills_its_buffer();
    pr3_a_frame_is_never_larger_than_its_buffer();
    pr3_the_frame_rows_are_kept_from_the_whole_buffer();
    pr3_a_frame_as_wide_as_its_buffer_is_left_as_it_is();
    if (failures != 0) {
        return EXIT_FAILURE;
    }
    std::puts("frame crop: all tests passed");
    return EXIT_SUCCESS;
}
