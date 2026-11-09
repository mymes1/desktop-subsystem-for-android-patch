// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "picture_copy.h"

#include <cstring>

namespace dsa::codec2 {

namespace {

// Repeats the last written sample of each row out to `toWidth`, and the last written row down
// to `toHeight`.
void extendPlane(const Plane& plane, uint32_t width, uint32_t height, uint32_t toWidth, uint32_t toHeight) {
    for (uint32_t row = 0; row < height; ++row) {
        uint8_t* line = plane.data + static_cast<size_t>(row) * plane.rowStride;
        const uint8_t edge = line[static_cast<size_t>(width - 1) * plane.pixelStride];
        for (uint32_t x = width; x < toWidth; ++x) {
            line[static_cast<size_t>(x) * plane.pixelStride] = edge;
        }
    }
    const uint8_t* last = plane.data + static_cast<size_t>(height - 1) * plane.rowStride;
    for (uint32_t row = height; row < toHeight; ++row) {
        uint8_t* line = plane.data + static_cast<size_t>(row) * plane.rowStride;
        for (uint32_t x = 0; x < toWidth; ++x) {
            line[static_cast<size_t>(x) * plane.pixelStride] = last[static_cast<size_t>(x) * plane.pixelStride];
        }
    }
}

}  // namespace

bool copyNv12(const uint8_t* nv12, size_t size, uint32_t width, uint32_t height, uint32_t stride,
              const YuvPlanes& planes) {
    return copyNv12(nv12, size, width, height, stride, planes, width, height);
}

bool copyNv12(const uint8_t* nv12, size_t size, uint32_t width, uint32_t height, uint32_t stride,
              const YuvPlanes& planes, uint32_t bufferWidth, uint32_t bufferHeight) {
    const size_t chromaRows = (height + 1) / 2;
    const size_t chromaWidth = (width + 1) / 2;
    // Each chroma row holds whole Cb, Cr pairs.
    if (width == 0 || height == 0 || stride < chromaWidth * 2 || bufferWidth < width || bufferHeight < height) {
        return false;
    }
    if (size < static_cast<size_t>(stride) * (height + chromaRows)) {
        return false;
    }
    for (uint32_t row = 0; row < height; ++row) {
        const uint8_t* src = nv12 + static_cast<size_t>(row) * stride;
        uint8_t* dst = planes.y.data + static_cast<size_t>(row) * planes.y.rowStride;
        if (planes.y.pixelStride == 1) {
            std::memcpy(dst, src, width);
        } else {
            for (uint32_t x = 0; x < width; ++x) {
                dst[x * planes.y.pixelStride] = src[x];
            }
        }
    }
    const uint8_t* chroma = nv12 + static_cast<size_t>(stride) * height;
    // Already NV12 (Cb then Cr interleaved): whole rows at once.
    const bool interleavedCbCr = planes.u.pixelStride == 2 && planes.v.pixelStride == 2 &&
                                 planes.v.data == planes.u.data + 1 && planes.u.rowStride == planes.v.rowStride;
    for (size_t row = 0; row < chromaRows; ++row) {
        const uint8_t* src = chroma + row * stride;
        uint8_t* u = planes.u.data + row * planes.u.rowStride;
        uint8_t* v = planes.v.data + row * planes.v.rowStride;
        if (interleavedCbCr) {
            std::memcpy(u, src, chromaWidth * 2);
            continue;
        }
        for (size_t x = 0; x < chromaWidth; ++x) {
            u[x * planes.u.pixelStride] = src[2 * x];
            v[x * planes.v.pixelStride] = src[2 * x + 1];
        }
    }
    // A buffer larger than the picture has its edge repeated rather than left as it was: a
    // consumer that shows more than the crop shows the picture's edge, never zeros (green).
    if (bufferWidth > width || bufferHeight > height) {
        extendPlane(planes.y, width, height, bufferWidth, bufferHeight);
        const uint32_t cw = static_cast<uint32_t>(chromaWidth);
        const uint32_t ch = static_cast<uint32_t>(chromaRows);
        const uint32_t toCw = (bufferWidth + 1) / 2;
        const uint32_t toCh = (bufferHeight + 1) / 2;
        extendPlane(planes.u, cw, ch, toCw, toCh);
        extendPlane(planes.v, cw, ch, toCw, toCh);
    }
    return true;
}

}  // namespace dsa::codec2
