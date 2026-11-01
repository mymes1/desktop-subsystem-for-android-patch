// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "shared_queue.h"

#include <linux/futex.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <climits>
#include <cstring>

namespace dsa::audio {

namespace {

constexpr size_t kPage = 4096;

int createMemory(size_t size) {
    const int fd = static_cast<int>(syscall(__NR_memfd_create, "dsa-audio-queue", MFD_CLOEXEC));
    if (fd < 0) {
        return -1;
    }
    if (ftruncate(fd, static_cast<off_t>(size)) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

}  // namespace

SharedQueue::SharedQueue(size_t quantum, size_t capacityBytes, bool blocking)
    : mQuantum(quantum), mCapacity(capacityBytes), mBlocking(blocking) {
    if (quantum == 0 || capacityBytes == 0 || capacityBytes % quantum != 0 ||
        capacityBytes > static_cast<size_t>(INT32_MAX) / 2) {
        return;
    }
    const size_t used = blocking ? eventWordOffset(capacityBytes) + sizeof(uint32_t) : 16 + capacityBytes;
    mMapSize = (used + kPage - 1) / kPage * kPage;
    mFd.reset(createMemory(mMapSize));
    if (!mFd.ok()) {
        return;
    }
    void* base = mmap(nullptr, mMapSize, PROT_READ | PROT_WRITE, MAP_SHARED, mFd.get(), 0);
    if (base == MAP_FAILED) {
        mFd.reset();
        return;
    }
    mBase = static_cast<uint8_t*>(base);
    // A new memfd reads as zeros: both counters start at 0 and the event word is clear.
    mReadCounter = reinterpret_cast<std::atomic<uint64_t>*>(mBase);
    mWriteCounter = reinterpret_cast<std::atomic<uint64_t>*>(mBase + 8);
    mRing = mBase + 16;
    if (blocking) {
        mEventWord = reinterpret_cast<std::atomic<uint32_t>*>(mBase + eventWordOffset(capacityBytes));
    }
}

SharedQueue::~SharedQueue() {
    if (mBase != nullptr) {
        munmap(mBase, mMapSize);
    }
}

Grantor SharedQueue::grantor(int index) const {
    switch (index) {
        case 0:
            return {0, 0, 8};
        case 1:
            return {0, 8, 8};
        case 2:
            return {0, 16, static_cast<int64_t>(mCapacity)};
        default:
            return {0, static_cast<int32_t>(eventWordOffset(mCapacity)), 4};
    }
}

size_t SharedQueue::availableToRead() const {
    if (!valid()) return 0;
    const uint64_t written = mWriteCounter->load(std::memory_order_acquire);
    const uint64_t read = mReadCounter->load(std::memory_order_relaxed);
    const uint64_t queued = written - read;
    // A peer that broke the counters gets nothing out of them.
    return queued > mCapacity ? 0 : static_cast<size_t>(queued);
}

size_t SharedQueue::availableToWrite() const {
    if (!valid()) return 0;
    const uint64_t written = mWriteCounter->load(std::memory_order_relaxed);
    const uint64_t read = mReadCounter->load(std::memory_order_acquire);
    const uint64_t queued = written - read;
    return queued > mCapacity ? 0 : mCapacity - static_cast<size_t>(queued);
}

size_t SharedQueue::read(void* out, size_t max) {
    const size_t count = std::min(max, availableToRead());
    if (count == 0) return 0;
    const uint64_t read = mReadCounter->load(std::memory_order_relaxed);
    const size_t at = static_cast<size_t>(read % mCapacity);
    const size_t first = std::min(count, mCapacity - at);
    std::memcpy(out, mRing + at, first);
    std::memcpy(static_cast<uint8_t*>(out) + first, mRing, count - first);
    mReadCounter->store(read + count, std::memory_order_release);
    return count;
}

bool SharedQueue::write(const void* in, size_t size) {
    if (size > availableToWrite()) return false;
    if (size == 0) return true;
    const uint64_t written = mWriteCounter->load(std::memory_order_relaxed);
    const size_t at = static_cast<size_t>(written % mCapacity);
    const size_t first = std::min(size, mCapacity - at);
    std::memcpy(mRing + at, in, first);
    std::memcpy(mRing, static_cast<const uint8_t*>(in) + first, size - first);
    mWriteCounter->store(written + size, std::memory_order_release);
    return true;
}

uint32_t SharedQueue::wait(uint32_t bits, int64_t timeoutNs) {
    if (mEventWord == nullptr || bits == 0) return 0;
    uint32_t old = mEventWord->fetch_and(~bits);
    if ((old & bits) != 0) {
        return old & bits;
    }
    timespec deadline{};
    clock_gettime(CLOCK_MONOTONIC, &deadline);
    const int64_t end = deadline.tv_nsec + timeoutNs;
    deadline.tv_sec += static_cast<time_t>(end / 1000000000);
    deadline.tv_nsec = static_cast<long>(end % 1000000000);
    // FUTEX_WAIT_BITSET takes an absolute CLOCK_MONOTONIC deadline. Not the private variant:
    // the word is shared with another process.
    syscall(__NR_futex, reinterpret_cast<uint32_t*>(mEventWord), FUTEX_WAIT_BITSET, old & ~bits,
            &deadline, nullptr, bits);
    old = mEventWord->fetch_and(~bits);
    return old & bits;
}

void SharedQueue::wake(uint32_t bits) {
    if (mEventWord == nullptr || bits == 0) return;
    const uint32_t old = mEventWord->fetch_or(bits);
    if ((~old & bits) != 0) {
        syscall(__NR_futex, reinterpret_cast<uint32_t*>(mEventWord), FUTEX_WAKE_BITSET, INT_MAX,
                nullptr, nullptr, bits);
    }
}

}  // namespace dsa::audio
