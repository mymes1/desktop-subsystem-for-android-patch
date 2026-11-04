// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The bytes waiting for the host's socket: frames go out whole and in order, a socket that
// takes part of them gets the rest later, and a host that stops reading costs new sound,
// never a wait.

#include <cstdint>
#include <vector>

#include "outbound.h"
#include "test_main.h"

using namespace dsa::audio;

TEST(pr4_a_socket_that_takes_everything_gets_every_frame_in_order) {
    Outbound out(100);
    EXPECT_TRUE(out.push({1, 2, 3}));
    EXPECT_TRUE(out.push({4, 5}));
    std::vector<uint8_t> wire;
    EXPECT_TRUE(out.flush([&](const uint8_t* data, size_t size) -> long {
        wire.insert(wire.end(), data, data + size);
        return static_cast<long>(size);
    }));
    EXPECT_TRUE(wire == (std::vector<uint8_t>{1, 2, 3, 4, 5}));
    EXPECT_EQ(out.size(), 0u);
}

TEST(pr4_what_the_socket_does_not_take_now_goes_next_time) {
    Outbound out(100);
    out.push({1, 2, 3, 4});
    std::vector<uint8_t> wire;
    // The socket takes two bytes, then would wait.
    int calls = 0;
    out.flush([&](const uint8_t* data, size_t) -> long {
        if (calls++ > 0) return 0;
        wire.insert(wire.end(), data, data + 2);
        return 2;
    });
    EXPECT_EQ(out.size(), 2u);
    out.push({5});
    out.flush([&](const uint8_t* data, size_t size) -> long {
        wire.insert(wire.end(), data, data + size);
        return static_cast<long>(size);
    });
    EXPECT_TRUE(wire == (std::vector<uint8_t>{1, 2, 3, 4, 5}));
}

TEST(pr4_a_host_that_stops_reading_drops_new_frames_whole) {
    Outbound out(6);
    EXPECT_TRUE(out.push({1, 2, 3, 4}));
    // Two more would fit, three do not: the frame is dropped, not cut.
    EXPECT_TRUE(!out.push({5, 6, 7}));
    EXPECT_EQ(out.size(), 4u);
    EXPECT_TRUE(out.push({5, 6}));
    EXPECT_EQ(out.size(), 6u);
}

TEST(pr4_a_failed_socket_is_reported) {
    Outbound out(10);
    out.push({1});
    EXPECT_TRUE(!out.flush([](const uint8_t*, size_t) -> long { return -1; }));
}

int main() { return dsa::test::runAll(); }
