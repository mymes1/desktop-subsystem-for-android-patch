// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The bytes waiting to go to the host on a socket the audio thread must never wait on. A frame
// goes in whole or not at all, so the host never sees half of one; what the socket does not
// take now stays for the next flush. When the host stops reading, the queue reaches its limit
// and new sound is dropped instead of piling up.

#pragma once

#include <cstddef>
#include <cstdint>
#include <functional>
#include <vector>

namespace dsa::audio {

class Outbound {
   public:
    // Holds at most `limit` bytes.
    explicit Outbound(size_t limit) : mLimit(limit) {}

    // Queues a whole frame (with its length). Returns false, queuing nothing, when it would
    // pass the limit.
    bool push(const std::vector<uint8_t>& frame);

    // Hands what is queued to `write`, which takes some prefix of what it is given and returns
    // how many bytes (0 when it would wait), or -1 when the connection failed. Returns false
    // on failure.
    bool flush(const std::function<long(const uint8_t*, size_t)>& write);

    size_t size() const { return mBytes.size() - mStart; }
    void clear() {
        mBytes.clear();
        mStart = 0;
    }

   private:
    size_t mLimit;
    std::vector<uint8_t> mBytes;
    size_t mStart = 0;
};

}  // namespace dsa::audio
