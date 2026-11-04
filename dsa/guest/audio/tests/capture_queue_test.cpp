// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The guest's queue of the host's recorded sound: it gives sound once it holds the target,
// silence when it runs dry, keeps the timeline across frames the host skipped, and never lets
// the delay grow past its limit.

#include <cstdint>
#include <vector>

#include "capture_queue.h"
#include "test_main.h"

using namespace dsa::audio;

namespace {

std::vector<uint8_t> frames(size_t count, uint8_t value) { return std::vector<uint8_t>(count * 4, value); }

std::vector<uint8_t> pull(CaptureQueue& queue, size_t count, size_t* sound = nullptr) {
    std::vector<uint8_t> out(count * 4, 0xee);
    const size_t got = queue.pull(out.data(), count);
    if (sound != nullptr) *sound = got;
    return out;
}

}  // namespace

TEST(fr26_recorded_sound_is_given_once_the_target_is_queued) {
    CaptureQueue queue(10, 100);
    const auto first = frames(6, 3);
    queue.push(0, first.data(), first.size());
    size_t sound = 99;
    const auto early = pull(queue, 2, &sound);
    EXPECT_EQ(sound, 0u);
    EXPECT_EQ(early[0], 0);
    const auto more = frames(4, 3);
    queue.push(6, more.data(), more.size());
    const auto out = pull(queue, 2, &sound);
    EXPECT_EQ(sound, 2u);
    EXPECT_EQ(out[0], 3);
    EXPECT_EQ(queue.bufferedFrames(), 8u);
}

TEST(fr26_an_empty_queue_gives_silence_and_waits_for_the_target_again) {
    CaptureQueue queue(4, 100);
    const auto data = frames(4, 9);
    queue.push(0, data.data(), data.size());
    size_t sound = 0;
    const auto out = pull(queue, 6, &sound);
    EXPECT_EQ(sound, 4u);
    EXPECT_EQ(out[15], 9);
    EXPECT_EQ(out[16], 0);
    EXPECT_EQ(queue.stats().underrunFrames, 2u);
    const auto little = frames(2, 9);
    queue.push(4, little.data(), little.size());
    pull(queue, 1, &sound);
    EXPECT_EQ(sound, 0u);
}

TEST(fr26_frames_the_host_skipped_become_silence) {
    CaptureQueue queue(1, 100);
    const auto a = frames(2, 5);
    queue.push(0, a.data(), a.size());
    queue.push(5, a.data(), a.size());
    EXPECT_EQ(queue.bufferedFrames(), 7u);
    EXPECT_EQ(queue.stats().gapFrames, 3u);
}

TEST(fr26_a_full_queue_drops_the_oldest_back_to_the_target) {
    CaptureQueue queue(10, 20);
    const auto old = frames(15, 1);
    const auto fresh = frames(10, 2);
    queue.push(0, old.data(), old.size());
    queue.push(15, fresh.data(), fresh.size());
    EXPECT_EQ(queue.bufferedFrames(), 10u);
    EXPECT_EQ(queue.stats().droppedFrames, 15u);
    const auto out = pull(queue, 10);
    EXPECT_EQ(out[0], 2);
}

TEST(fr26_a_piece_that_is_not_whole_frames_is_refused) {
    CaptureQueue queue(1, 100);
    const uint8_t odd[3] = {1, 2, 3};
    EXPECT_TRUE(!queue.push(0, odd, sizeof(odd)));
    EXPECT_EQ(queue.bufferedFrames(), 0u);
}

TEST(fr26_clearing_starts_a_new_timeline) {
    CaptureQueue queue(1, 100);
    const auto a = frames(2, 5);
    queue.push(0, a.data(), a.size());
    queue.clear();
    queue.push(1000, a.data(), a.size());
    EXPECT_EQ(queue.bufferedFrames(), 2u);
    EXPECT_EQ(queue.stats().gapFrames, 0u);
}

int main() { return dsa::test::runAll(); }
