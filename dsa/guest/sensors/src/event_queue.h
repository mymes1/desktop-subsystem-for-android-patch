// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The writing end of the framework's sensor event queue: a fast message queue
// (android.hardware.common.fmq, synchronized read-write) the framework makes and hands the
// HAL in initialize. libfmq is not in the NDK, so this is its writer's part, on memory the
// caller has mapped:
//
// - a 64-bit read counter and a 64-bit write counter, in bytes, which only grow;
// - a ring of `capacity` bytes the events are copied into, at the write counter modulo the
//   capacity;
// - a 32-bit event flag word, a futex: the writer sets the "read and process" bit on it and
//   wakes the reader.
//
// The writer never moves past the reader: events that do not fit are dropped, as the
// framework would have dropped them reading late.

#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <functional>

namespace dsa::sensors {

class EventQueue {
   public:
    // Wakes the reader waiting on the flag word for `bits`. The default is the futex wake
    // libfmq's EventFlag does.
    using Waker = std::function<void(std::atomic<uint32_t>* word, uint32_t bits)>;

    EventQueue(std::atomic<uint64_t>* readCounter, std::atomic<uint64_t>* writeCounter, uint8_t* ring,
               size_t capacity, size_t quantum, std::atomic<uint32_t>* flagWord, Waker waker = futexWake);

    // Copies as many of the `count` items of `quantum` bytes at `items` as fit, in order, and
    // returns how many it copied. Nothing is visible to the reader until it returns.
    size_t write(const void* items, size_t count);

    // Sets `bits` on the flag word and wakes the reader, if it has a flag word.
    void wake(uint32_t bits);

    size_t capacityItems() const { return mCapacity / mQuantum; }

    static void futexWake(std::atomic<uint32_t>* word, uint32_t bits);

   private:
    std::atomic<uint64_t>* mRead;
    std::atomic<uint64_t>* mWrite;
    uint8_t* mRing;
    size_t mCapacity;
    size_t mQuantum;
    std::atomic<uint32_t>* mFlag;
    Waker mWaker;
};

}  // namespace dsa::sensors
