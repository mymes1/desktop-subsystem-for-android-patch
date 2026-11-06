// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "location_session.h"

namespace dsa::gnss {

std::vector<uint8_t> LocationSession::start() {
    mRunning = true;
    return encodeLocationRequest(true, mIntervalMs);
}

std::optional<std::vector<uint8_t>> LocationSession::stop() {
    if (!mRunning) {
        return std::nullopt;
    }
    mRunning = false;
    return encodeLocationRequest(false, 0);
}

std::optional<std::vector<uint8_t>> LocationSession::setInterval(uint32_t intervalMs) {
    const uint32_t interval =
        intervalMs == 0 ? kDefaultIntervalMs : (intervalMs < kMinIntervalMs ? kMinIntervalMs : intervalMs);
    if (interval == mIntervalMs) {
        return std::nullopt;
    }
    mIntervalMs = interval;
    if (!mRunning) {
        return std::nullopt;
    }
    return encodeLocationRequest(true, mIntervalMs);
}

std::optional<std::vector<uint8_t>> LocationSession::reconnected() const {
    if (!mRunning) {
        return std::nullopt;
    }
    return encodeLocationRequest(true, mIntervalMs);
}

std::optional<Fix> LocationSession::fromHost(const HostLocation& location) {
    if (location.access) {
        mAccess = *location.access;
    }
    if (!location.fix || !mRunning || mAccess == Access::Denied || mAccess == Access::Unavailable) {
        return std::nullopt;
    }
    return location.fix;
}

}  // namespace dsa::gnss
