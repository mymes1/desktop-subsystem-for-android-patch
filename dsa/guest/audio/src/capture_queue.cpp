// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "capture_queue.h"

#include <algorithm>
#include <cstring>

namespace dsa::audio {

CaptureQueue::CaptureQueue(size_t targetFrames, size_t maxFrames)
    : mTargetFrames(targetFrames), mMaxFrames(std::max(maxFrames, targetFrames)) {}

bool CaptureQueue::push(uint64_t position, const uint8_t* data, size_t size) {
    if (size % kFrameBytes != 0) {
        return false;
    }
    std::lock_guard<std::mutex> lock(mLock);
    size_t frames = size / kFrameBytes;
    size_t skip = 0;
    if (mHasNext) {
        if (position > mNextPosition) {
            // The host skipped frames: keep the timeline with silence, but never more than
            // the queue can hold.
            const size_t gap =
                    static_cast<size_t>(std::min<uint64_t>(position - mNextPosition, mMaxFrames));
            mBytes.insert(mBytes.end(), gap * kFrameBytes, 0);
            mStats.gapFrames += gap;
        } else if (position < mNextPosition) {
            skip = static_cast<size_t>(std::min<uint64_t>(mNextPosition - position, frames));
        }
    }
    mStats.receivedFrames += frames;
    const uint64_t end = position + frames;
    mNextPosition = mHasNext ? std::max(mNextPosition, end) : end;
    mHasNext = true;
    frames -= skip;
    const uint8_t* start = data + skip * kFrameBytes;
    mBytes.insert(mBytes.end(), start, start + frames * kFrameBytes);
    const size_t queued = mBytes.size() / kFrameBytes;
    if (queued > mMaxFrames) {
        const size_t drop = queued - mTargetFrames;
        mBytes.erase(mBytes.begin(), mBytes.begin() + static_cast<std::ptrdiff_t>(drop * kFrameBytes));
        mStats.droppedFrames += drop;
    }
    if (mBytes.size() / kFrameBytes >= mTargetFrames) {
        mPrimed = true;
    }
    return true;
}

size_t CaptureQueue::pull(uint8_t* out, size_t frames) {
    std::lock_guard<std::mutex> lock(mLock);
    size_t have = 0;
    if (mPrimed) {
        have = std::min(mBytes.size() / kFrameBytes, frames);
        std::copy(mBytes.begin(), mBytes.begin() + static_cast<std::ptrdiff_t>(have * kFrameBytes), out);
        mBytes.erase(mBytes.begin(), mBytes.begin() + static_cast<std::ptrdiff_t>(have * kFrameBytes));
        if (have < frames) {
            mStats.underrunFrames += frames - have;
            mPrimed = false;
        }
    }
    std::memset(out + have * kFrameBytes, 0, (frames - have) * kFrameBytes);
    return have;
}

void CaptureQueue::clear() {
    std::lock_guard<std::mutex> lock(mLock);
    mBytes.clear();
    mPrimed = false;
    mHasNext = false;
}

size_t CaptureQueue::bufferedFrames() const {
    std::lock_guard<std::mutex> lock(mLock);
    return mBytes.size() / kFrameBytes;
}

CaptureStats CaptureQueue::stats() const {
    std::lock_guard<std::mutex> lock(mLock);
    return mStats;
}

}  // namespace dsa::audio
