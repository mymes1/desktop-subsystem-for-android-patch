// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#pragma once

#include <unistd.h>

#include <utility>

namespace dsa::audio {

// An owned file descriptor, closed when it goes away.
class UniqueFd {
   public:
    UniqueFd() = default;
    explicit UniqueFd(int fd) : mFd(fd) {}
    ~UniqueFd() { reset(); }
    UniqueFd(UniqueFd&& other) noexcept : mFd(other.release()) {}
    UniqueFd& operator=(UniqueFd&& other) noexcept {
        if (this != &other) {
            reset(other.release());
        }
        return *this;
    }
    UniqueFd(const UniqueFd&) = delete;
    UniqueFd& operator=(const UniqueFd&) = delete;

    int get() const { return mFd; }
    bool ok() const { return mFd >= 0; }
    int release() { return std::exchange(mFd, -1); }
    void reset(int fd = -1) {
        if (mFd >= 0) {
            close(mFd);
        }
        mFd = fd;
    }

   private:
    int mFd = -1;
};

}  // namespace dsa::audio
