// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "refresh_rates.h"

namespace dsa::hwc {

bool isRefreshConfig(int32_t config) { return config >= 0 && config < kRefreshConfigCount; }

int32_t refreshRateOf(int32_t config) { return kRefreshRatesHz[config]; }

int32_t vsyncPeriodOf(int32_t config) { return 1000000000 / kRefreshRatesHz[config]; }

int32_t configForRate(int32_t hz) {
    int32_t chosen = 0;
    for (int32_t config = 0; config < kRefreshConfigCount; ++config) {
        if (kRefreshRatesHz[config] <= hz) {
            chosen = config;
        }
    }
    return chosen;
}

}  // namespace dsa::hwc
