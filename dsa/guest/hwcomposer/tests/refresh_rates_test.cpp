// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The display's refresh rates: one configuration per rate the host can choose, so the host's
// battery saver can take a 120 Hz display to 60 Hz and back while Android runs.

#include "refresh_rates.h"
#include "test_main.h"

using namespace dsa::hwc;

TEST(fr10_battery_saver_caps_the_guest_refresh_rate_through_a_configuration_of_its_own) {
    // The host's rates under battery saver and without it on a 120 Hz monitor: both are
    // configurations the display offers, so SurfaceFlinger can move between them.
    const int32_t sixty = configForRate(60);
    const int32_t oneTwenty = configForRate(120);
    EXPECT_TRUE(isRefreshConfig(sixty));
    EXPECT_TRUE(isRefreshConfig(oneTwenty));
    EXPECT_EQ(refreshRateOf(sixty), 60);
    EXPECT_EQ(refreshRateOf(oneTwenty), 120);
    EXPECT_EQ(vsyncPeriodOf(sixty), 16666666);
    EXPECT_EQ(vsyncPeriodOf(oneTwenty), 8333333);
}

TEST(fr10_every_rate_the_host_chooses_is_a_configuration) {
    for (int32_t hz : {60, 75, 90, 100, 120, 144, 165, 240}) {
        EXPECT_EQ(refreshRateOf(configForRate(hz)), hz);
    }
}

TEST(fr10_a_boot_rate_between_rates_starts_at_the_one_below) {
    EXPECT_EQ(refreshRateOf(configForRate(59)), 60);
    EXPECT_EQ(refreshRateOf(configForRate(85)), 75);
    EXPECT_EQ(refreshRateOf(configForRate(500)), 240);
    EXPECT_TRUE(!isRefreshConfig(-1));
    EXPECT_TRUE(!isRefreshConfig(kRefreshConfigCount));
}

int main() { return dsa::test::runAll(); }
