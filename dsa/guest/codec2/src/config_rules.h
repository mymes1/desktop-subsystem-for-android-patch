// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// What the decoders accept when the framework configures them.
//
// The framework (CCodecConfig) sets the output's pixel format from the app's color format:
// HAL_PIXEL_FORMAT_YCBCR_420_888 for an app that reads the pictures itself, and
// HAL_PIXEL_FORMAT_IMPLEMENTATION_DEFINED for an app that shows them on a surface
// (COLOR_FormatSurface). The pixel format was a constant, and the platform's interface helper
// answers a constant set to another value with C2_CORRUPTED, so every configure of a decoder
// for a surface ended in "config failed => CORRUPTED". A surface takes whatever the decoder
// gives it, so that request is met with the flexible format the decoder always outputs, and
// the configure succeeds; any other format is a bad value, as the helper reports one.

#pragma once

#include <cstdint>

namespace dsa::codec2 {

// HAL_PIXEL_FORMAT_YCBCR_420_888: flexible YUV 4:2:0, what the decoders output.
inline constexpr uint32_t kPixelFormatFlexible = 0x23;
// HAL_PIXEL_FORMAT_IMPLEMENTATION_DEFINED: what the framework asks for on a surface.
inline constexpr uint32_t kPixelFormatSurface = 0x22;

// Whether a decoder takes `requested` as its output pixel format, and the format it keeps
// (`*kept`) when it does.
bool acceptPixelFormat(uint32_t requested, uint32_t* kept);

}  // namespace dsa::codec2
