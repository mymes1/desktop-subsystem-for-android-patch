// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "outbound.h"

namespace dsa::audio {

bool Outbound::push(const std::vector<uint8_t>& frame) {
    if (size() + frame.size() > mLimit) {
        return false;
    }
    if (mStart > 0 && mStart == mBytes.size()) {
        clear();
    }
    mBytes.insert(mBytes.end(), frame.begin(), frame.end());
    return true;
}

bool Outbound::flush(const std::function<long(const uint8_t*, size_t)>& write) {
    while (size() > 0) {
        const long n = write(mBytes.data() + mStart, size());
        if (n < 0) {
            return false;
        }
        if (n == 0) {
            break;
        }
        mStart += static_cast<size_t>(n);
    }
    if (mStart == mBytes.size()) {
        clear();
    } else if (mStart > mLimit) {
        // Keep the vector from growing without bound while the front drains.
        mBytes.erase(mBytes.begin(), mBytes.begin() + static_cast<long>(mStart));
        mStart = 0;
    }
    return true;
}

}  // namespace dsa::audio
