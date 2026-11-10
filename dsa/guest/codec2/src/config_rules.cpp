// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "config_rules.h"

namespace dsa::codec2 {

bool acceptPixelFormat(uint32_t requested, uint32_t* kept) {
    if (requested == kPixelFormatFlexible || requested == kPixelFormatSurface) {
        *kept = kPixelFormatFlexible;
        return true;
    }
    return false;
}

}  // namespace dsa::codec2
