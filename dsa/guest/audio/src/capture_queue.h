// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The guest's queue of what the host's microphone recorded, in the stream's format (48 kHz,
// stereo, 16-bit). The link's thread puts in what arrives, at the host device's pace; the
// input stream takes it out at the guest's own pace. It starts giving sound once it holds the
// target, and after it runs dry it waits to hold that much again, so a late delivery costs
// one short silence instead of a gap in every burst after it. When it passes its limit (the
// two clocks drift apart, or nobody reads), the oldest sound goes and it is back at the
// target, so the delay never grows.

#pragma once

#include <cstddef>
#include <cstdint>
#include <deque>
#include <mutex>

namespace dsa::audio {

struct CaptureStats {
    // Frames the host sent.
    uint64_t receivedFrames = 0;
    // Frames of silence the stream gave because the queue was empty.
    uint64_t underrunFrames = 0;
    // Frames dropped because the queue was full.
    uint64_t droppedFrames = 0;
    // Frames of silence put in for frames the host skipped.
    uint64_t gapFrames = 0;
};

class CaptureQueue {
   public:
    static constexpr size_t kFrameBytes = 4;

    // A queue that starts giving sound at `targetFrames` and holds at most `maxFrames` (at
    // least the target).
    CaptureQueue(size_t targetFrames, size_t maxFrames);

    // Queues `size` bytes of whole frames, which start `position` frames into the stream.
    // Frames the host skipped become silence; frames it sends again are dropped. Returns false,
    // queuing nothing, when `size` is not whole frames.
    bool push(uint64_t position, const uint8_t* data, size_t size);

    // Fills `out` with `frames` frames: what is queued, then silence. Never waits. Returns how
    // many of them were recorded sound.
    size_t pull(uint8_t* out, size_t frames);

    // Drops what is queued; the next push starts a new timeline.
    void clear();

    size_t bufferedFrames() const;
    CaptureStats stats() const;

   private:
    mutable std::mutex mLock;
    std::deque<uint8_t> mBytes;
    const size_t mTargetFrames;
    const size_t mMaxFrames;
    bool mPrimed = false;
    bool mHasNext = false;
    uint64_t mNextPosition = 0;
    CaptureStats mStats;
};

}  // namespace dsa::audio
