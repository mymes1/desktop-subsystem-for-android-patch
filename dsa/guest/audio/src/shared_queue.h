// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// A single-producer, single-consumer queue in shared memory, laid out the way the platform's
// fast message queue (libfmq) lays out a synchronized queue, so the audio framework's own
// libfmq reads and writes it on the other side. libfmq is not part of the NDK, so the HAL
// keeps its own copy of the layout:
//
//   offset 0                 read counter   (uint64, bytes read since the queue was made)
//   offset 8                 write counter  (uint64, bytes written)
//   offset 16                the ring       (capacity bytes)
//   offset align8(16 + cap)  event word     (uint32, only for queues that block)
//
// The counters only grow; the position in the ring is the counter modulo the capacity. A
// reader that blocks waits on the event word for the "not empty" bit, which the writer sets
// after each write; a writer that blocks waits for "not full", which the reader sets. Both
// sides wake each other with futexes on that shared word.

#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>

#include "unique_fd.h"

namespace dsa::audio {

// The bits of the event word, as libfmq defines them.
constexpr uint32_t kQueueNotFull = 0x01;
constexpr uint32_t kQueueNotEmpty = 0x02;

// How libfmq marks a queue's flavor in its descriptor's flags.
constexpr int32_t kSynchronizedReadWrite = 0x01;

// One region of the queue's memory, as a descriptor names it.
struct Grantor {
    int32_t fdIndex;
    int32_t offset;
    int64_t extent;
};

class SharedQueue {
   public:
    // A queue of `capacityBytes` (whole elements of `quantum` bytes). With `blocking`, it has
    // the event word blocking readers and writers wait on. Check valid() afterwards.
    SharedQueue(size_t quantum, size_t capacityBytes, bool blocking);
    ~SharedQueue();
    SharedQueue(const SharedQueue&) = delete;
    SharedQueue& operator=(const SharedQueue&) = delete;

    bool valid() const { return mBase != nullptr; }

    // What a descriptor of this queue holds: the memory's file descriptor (to be duplicated
    // into the descriptor), its regions, the element size and the flavor flags.
    int fd() const { return mFd.get(); }
    int grantorCount() const { return mBlocking ? 4 : 3; }
    Grantor grantor(int index) const;
    int32_t quantum() const { return static_cast<int32_t>(mQuantum); }
    size_t capacity() const { return mCapacity; }

    size_t availableToRead() const;
    size_t availableToWrite() const;

    // Copies up to `max` bytes out. Returns how many.
    size_t read(void* out, size_t max);
    // Copies `size` bytes in, or nothing when they do not fit. Returns whether it did.
    bool write(const void* in, size_t size);

    // Waits at most `timeoutNs` for any of `bits` to be set on the event word, clears them
    // and returns those that were set (0 on a timeout). Only for blocking queues.
    uint32_t wait(uint32_t bits, int64_t timeoutNs);
    // Sets `bits` on the event word and wakes whoever waits for them.
    void wake(uint32_t bits);

   private:
    UniqueFd mFd;
    size_t mQuantum;
    size_t mCapacity;
    bool mBlocking;
    size_t mMapSize = 0;
    uint8_t* mBase = nullptr;
    std::atomic<uint64_t>* mReadCounter = nullptr;
    std::atomic<uint64_t>* mWriteCounter = nullptr;
    uint8_t* mRing = nullptr;
    std::atomic<uint32_t>* mEventWord = nullptr;
};

// The offset of the event word: after both counters and the ring, aligned to 8 bytes.
constexpr size_t eventWordOffset(size_t capacityBytes) {
    return (16 + capacityBytes + 7) & ~static_cast<size_t>(7);
}

}  // namespace dsa::audio
