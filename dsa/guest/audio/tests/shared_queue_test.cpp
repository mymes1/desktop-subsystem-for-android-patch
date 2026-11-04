// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The shared queues the framework talks to a stream through, laid out as the platform's
// fast message queue lays out a synchronized queue: the counters, the ring and the event
// word sit where its descriptor says, sound wraps around the ring intact, and a waiter wakes
// when the other side sets its bit.

#include <sys/mman.h>

#include <atomic>
#include <chrono>
#include <cstring>
#include <thread>
#include <vector>

#include "shared_queue.h"
#include "test_main.h"

using namespace dsa::audio;

TEST(pr4_the_descriptor_names_counters_ring_and_event_word) {
    SharedQueue queue(8, 64, true);
    EXPECT_TRUE(queue.valid());
    EXPECT_EQ(queue.grantorCount(), 4);
    EXPECT_EQ(queue.grantor(0).offset, 0);
    EXPECT_EQ(queue.grantor(1).offset, 8);
    EXPECT_EQ(queue.grantor(2).offset, 16);
    EXPECT_EQ(queue.grantor(2).extent, 64);
    EXPECT_EQ(queue.grantor(3).offset, 80);
    EXPECT_EQ(queue.quantum(), 8);

    SharedQueue data(1, 100, false);
    EXPECT_EQ(data.grantorCount(), 3);
}

TEST(pr4_sound_wraps_around_the_ring_intact) {
    SharedQueue queue(1, 10, false);
    std::vector<uint8_t> out(10);
    const uint8_t first[7] = {1, 2, 3, 4, 5, 6, 7};
    EXPECT_TRUE(queue.write(first, sizeof(first)));
    EXPECT_EQ(queue.read(out.data(), 7), 7u);
    const uint8_t second[8] = {10, 11, 12, 13, 14, 15, 16, 17};
    EXPECT_TRUE(queue.write(second, sizeof(second)));
    EXPECT_EQ(queue.availableToRead(), 8u);
    EXPECT_EQ(queue.read(out.data(), 10), 8u);
    EXPECT_EQ(std::memcmp(out.data(), second, 8), 0);
}

TEST(pr4_a_write_that_does_not_fit_writes_nothing) {
    SharedQueue queue(1, 4, false);
    const uint8_t bytes[5] = {};
    EXPECT_TRUE(!queue.write(bytes, 5));
    EXPECT_EQ(queue.availableToRead(), 0u);
}

TEST(pr4_the_other_side_sees_writes_through_the_shared_memory) {
    // The framework maps the same memory from the descriptor's file and offsets.
    SharedQueue queue(1, 16, false);
    void* peer = mmap(nullptr, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, queue.fd(), 0);
    EXPECT_TRUE(peer != MAP_FAILED);
    if (peer == MAP_FAILED) return;
    auto* base = static_cast<uint8_t*>(peer);
    // The peer writes three bytes the way libfmq does: the ring, then the write counter.
    base[16] = 'a';
    base[17] = 'b';
    base[18] = 'c';
    reinterpret_cast<std::atomic<uint64_t>*>(base + 8)->store(3);
    uint8_t out[3] = {};
    EXPECT_EQ(queue.read(out, 3), 3u);
    EXPECT_EQ(out[2], 'c');
    EXPECT_EQ(reinterpret_cast<std::atomic<uint64_t>*>(base)->load(), 3u);
    munmap(peer, 4096);
}

TEST(pr4_a_waiter_wakes_when_its_bit_is_set) {
    SharedQueue queue(4, 16, true);
    std::atomic<uint32_t> got{0};
    std::thread waiter([&] { got = queue.wait(kQueueNotEmpty, 2000LL * 1000 * 1000); });
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
    queue.wake(kQueueNotEmpty);
    waiter.join();
    EXPECT_EQ(got.load(), kQueueNotEmpty);
    // A wait with nothing set times out.
    EXPECT_EQ(queue.wait(kQueueNotFull, 1000 * 1000), 0u);
}

int main() { return dsa::test::runAll(); }
