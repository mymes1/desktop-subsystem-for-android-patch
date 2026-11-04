// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The GNSS HAL's rules, apart from binder: when the host is asked for locations, and which of
// the host's positions reach the framework. The framework starts and stops the GNSS engine;
// the host runs its location service only between the two. A host that is not allowed to read
// its location sends no position, and one that arrives anyway, or after the engine stopped,
// is dropped. Not thread safe: the service holds its own lock around every call.

#pragma once

#include <cstdint>
#include <optional>
#include <vector>

#include "location_messages.h"

namespace dsa::gnss {

class LocationSession {
   public:
    // How often the framework gets a location when it does not say: once a second, as a
    // phone's GNSS gives one.
    static constexpr uint32_t kDefaultIntervalMs = 1000;
    // The shortest interval the host is asked for.
    static constexpr uint32_t kMinIntervalMs = 100;

    // The framework started the engine. Returns the request for the host.
    std::vector<uint8_t> start();
    // The framework stopped the engine. Returns the request for the host, or nothing when the
    // engine was not running.
    std::optional<std::vector<uint8_t>> stop();
    // The framework set how often it wants a location. Returns the request for the host when
    // the engine runs and the interval changed.
    std::optional<std::vector<uint8_t>> setInterval(uint32_t intervalMs);
    // The control channel subscribed again. Returns the request the host must hear again,
    // when the engine runs.
    std::optional<std::vector<uint8_t>> reconnected() const;

    // What the host said. Returns the position to hand the framework, if it is one that may
    // reach it now.
    std::optional<Fix> fromHost(const HostLocation& location);

    bool running() const { return mRunning; }
    uint32_t intervalMs() const { return mIntervalMs; }
    Access access() const { return mAccess; }

   private:
    bool mRunning = false;
    uint32_t mIntervalMs = kDefaultIntervalMs;
    Access mAccess = Access::Unknown;
};

}  // namespace dsa::gnss
