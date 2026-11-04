// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "event_queue.h"

#include <linux/futex.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <algorithm>
#include <climits>
#include <cstring>
#include <utility>

namespace dsa::sensors {

EventQueue::EventQueue(std::atomic<uint64_t>* readCounter, std::atomic<uint64_t>* writeCounter, uint8_t* ring,
                       size_t capacity, size_t quantum, std::atomic<uint32_t>* flagWord, Waker waker)
    : mRead(readCounter),
      mWrite(writeCounter),
      mRing(ring),
      mCapacity(capacity),
      mQuantum(quantum == 0 ? 1 : quantum),
      mFlag(flagWord),
      mWaker(std::move(waker)) {}

size_t EventQueue::write(const void* items, size_t count) {
    if (mCapacity == 0 || count == 0) {
        return 0;
    }
    const uint64_t written = mWrite->load(std::memory_order_relaxed);
    const uint64_t read = mRead->load(std::memory_order_acquire);
    const uint64_t used = written - read;
    if (used >= mCapacity) {
        return 0;
    }
    const size_t room = static_cast<size_t>(mCapacity - used) / mQuantum;
    const size_t fitting = std::min(count, room);
    const size_t bytes = fitting * mQuantum;
    const auto* source = static_cast<const uint8_t*>(items);
    const size_t start = static_cast<size_t>(written % mCapacity);
    const size_t first = std::min(bytes, mCapacity - start);
    std::memcpy(mRing + start, source, first);
    if (bytes > first) {
        std::memcpy(mRing, source + first, bytes - first);
    }
    mWrite->store(written + bytes, std::memory_order_release);
    return fitting;
}

void EventQueue::wake(uint32_t bits) {
    if (mFlag != nullptr && mWaker) {
        mWaker(mFlag, bits);
    }
}

void EventQueue::futexWake(std::atomic<uint32_t>* word, uint32_t bits) {
    const uint32_t old = word->fetch_or(bits);
    if ((~old & bits) != 0) {
        syscall(SYS_futex, reinterpret_cast<uint32_t*>(word), FUTEX_WAKE_BITSET, INT_MAX, nullptr, nullptr, bits);
    }
}

}  // namespace dsa::sensors
