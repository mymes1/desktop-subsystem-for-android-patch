// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The refresh rates the display offers: one display configuration each, the same size and
// in one configuration group, so SurfaceFlinger switches between them seamlessly, without a
// new display or a restart. The host chooses the rate from the user's fastest monitor and
// its battery saver (dsa/host/core/src/guest_refresh.rs lists the same rates), the agent
// holds the framework's refresh rate range at it, and SurfaceFlinger then sets the matching
// configuration here.

#pragma once

#include <cstddef>
#include <cstdint>

namespace dsa::hwc {

// The rates, slowest first; a configuration's ID is its index.
constexpr int32_t kRefreshRatesHz[] = {60, 75, 90, 100, 120, 144, 165, 240};
constexpr int32_t kRefreshConfigCount =
    static_cast<int32_t>(sizeof(kRefreshRatesHz) / sizeof(kRefreshRatesHz[0]));

// Whether `config` is one of the display's configurations.
bool isRefreshConfig(int32_t config);

// The configuration's rate in hertz, and its vsync period in nanoseconds. Both are for a
// valid configuration only.
int32_t refreshRateOf(int32_t config);
int32_t vsyncPeriodOf(int32_t config);

// The configuration the display starts in for a boot rate of `hz`: the fastest at or below
// it, and the slowest for a rate below every one.
int32_t configForRate(int32_t hz);

}  // namespace dsa::hwc
