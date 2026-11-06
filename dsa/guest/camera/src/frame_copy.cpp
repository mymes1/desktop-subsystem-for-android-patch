// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "frame_copy.h"

#include <algorithm>
#include <cstring>

namespace dsa::camera {

namespace {

// The JPEG blob's trailer: CameraBlobId::JPEG and the JPEG's length, as two 32-bit integers.
constexpr int32_t kJpegBlobId = 0x00FF;

uint8_t clamp(int value) { return static_cast<uint8_t>(std::min(255, std::max(0, (value + 128) >> 8))); }

}  // namespace

bool isWhole(const Nv12& picture) {
    if (picture.data == nullptr || picture.width == 0 || picture.height == 0) {
        return false;
    }
    if (picture.stride < picture.width + (picture.width & 1)) {
        return false;
    }
    const size_t rows = picture.height + (picture.height + 1) / 2;
    return picture.size >= static_cast<size_t>(picture.stride) * rows;
}

bool scaleToYuv(const Nv12& picture, uint32_t width, uint32_t height, const YuvPlanes& planes) {
    if (!isWhole(picture) || width == 0 || height == 0) {
        return false;
    }
    const uint32_t sw = picture.width;
    const uint32_t sh = picture.height;
    const size_t ss = picture.stride;
    for (uint32_t y = 0; y < height; ++y) {
        const uint8_t* src = picture.data + static_cast<size_t>(y) * sh / height * ss;
        uint8_t* dst = planes.y.data + static_cast<size_t>(y) * planes.y.rowStride;
        if (sw == width && planes.y.pixelStride == 1) {
            std::memcpy(dst, src, width);
            continue;
        }
        for (uint32_t x = 0; x < width; ++x) {
            dst[static_cast<size_t>(x) * planes.y.pixelStride] = src[static_cast<size_t>(x) * sw / width];
        }
    }
    const uint8_t* chroma = picture.data + ss * sh;
    const uint32_t scw = (sw + 1) / 2;
    const uint32_t sch = (sh + 1) / 2;
    const uint32_t dcw = (width + 1) / 2;
    const uint32_t dch = (height + 1) / 2;
    for (uint32_t y = 0; y < dch; ++y) {
        const uint32_t sy = std::min(sch - 1, static_cast<uint32_t>(static_cast<uint64_t>(y) * sch / dch));
        const uint8_t* src = chroma + static_cast<size_t>(sy) * ss;
        uint8_t* u = planes.u.data + static_cast<size_t>(y) * planes.u.rowStride;
        uint8_t* v = planes.v.data + static_cast<size_t>(y) * planes.v.rowStride;
        for (uint32_t x = 0; x < dcw; ++x) {
            const uint32_t sx = std::min(scw - 1, static_cast<uint32_t>(static_cast<uint64_t>(x) * scw / dcw));
            u[static_cast<size_t>(x) * planes.u.pixelStride] = src[2 * sx];
            v[static_cast<size_t>(x) * planes.v.pixelStride] = src[2 * sx + 1];
        }
    }
    return true;
}

bool scaleToRgba(const Nv12& picture, uint32_t width, uint32_t height, uint8_t* rgba, uint32_t rowBytes) {
    if (!isWhole(picture) || width == 0 || height == 0 || rowBytes < width * 4) {
        return false;
    }
    const uint32_t sw = picture.width;
    const uint32_t sh = picture.height;
    const size_t ss = picture.stride;
    const uint8_t* chroma = picture.data + ss * sh;
    for (uint32_t y = 0; y < height; ++y) {
        const size_t sy = static_cast<size_t>(y) * sh / height;
        uint8_t* out = rgba + static_cast<size_t>(y) * rowBytes;
        for (uint32_t x = 0; x < width; ++x) {
            const size_t sx = static_cast<size_t>(x) * sw / width;
            const int luma = picture.data[sy * ss + sx];
            const size_t at = (sy / 2) * ss + (sx / 2) * 2;
            const int u = chroma[at] - 128;
            const int v = chroma[at + 1] - 128;
            const int c = 298 * (luma - 16);
            out[4 * x] = clamp(c + 409 * v);
            out[4 * x + 1] = clamp(c - 100 * u - 208 * v);
            out[4 * x + 2] = clamp(c + 516 * u);
            out[4 * x + 3] = 255;
        }
    }
    return true;
}

bool writeJpegBlob(uint8_t* buffer, size_t bufferSize, const uint8_t* jpeg, size_t jpegSize) {
    constexpr size_t kTrailer = 2 * sizeof(int32_t);
    if (buffer == nullptr || bufferSize < kTrailer || jpegSize > bufferSize - kTrailer) {
        return false;
    }
    std::memcpy(buffer, jpeg, jpegSize);
    const int32_t trailer[2] = {kJpegBlobId, static_cast<int32_t>(jpegSize)};
    std::memcpy(buffer + bufferSize - kTrailer, trailer, kTrailer);
    return true;
}

}  // namespace dsa::camera
