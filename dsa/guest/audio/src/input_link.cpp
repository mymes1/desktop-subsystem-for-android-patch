// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "input_link.h"

#include <sys/socket.h>
#include <sys/time.h>

#include <chrono>
#include <variant>
#include <vector>

#include "audio_messages.h"
#include "log.h"
#include "socket_io.h"

namespace dsa::audio {

namespace {

// 20 ms: the host's device delivers 10 ms at a time, so this covers one late delivery.
constexpr size_t kTargetFrames = 48000 / 50;
// 100 ms: past this the oldest sound goes, so the delay never grows.
constexpr size_t kMaxFrames = 48000 / 10;
constexpr auto kRetryDelay = std::chrono::seconds(1);

void setTimeouts(int fd, int seconds) {
    timeval timeout{seconds, 0};
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
}

}  // namespace

InputLink::InputLink(std::string name, uint32_t framesPerBurst, std::function<int()> connect)
    : mName(std::move(name)),
      mFramesPerBurst(framesPerBurst),
      mConnect(std::move(connect)),
      mQueue(kTargetFrames, kMaxFrames) {}

std::unique_ptr<InputLink> InputLink::open(std::string name, uint32_t framesPerBurst,
                                           std::function<int()> connect, std::string* error) {
    auto link = std::make_unique<InputLink>(std::move(name), framesPerBurst, std::move(connect));
    UniqueFd fd(link->mConnect());
    if (!fd.ok()) {
        *error = "the host's audio port is not answering";
        return nullptr;
    }
    if (!link->handshake(fd.get(), error)) {
        return nullptr;
    }
    const int socket = fd.release();
    link->mSocket = socket;
    link->mThread = std::thread([raw = link.get(), socket] { raw->run(socket); });
    return link;
}

InputLink::~InputLink() {
    {
        std::lock_guard<std::mutex> lock(mLock);
        mStopping = true;
        if (mSocket >= 0) {
            shutdown(mSocket, SHUT_RDWR);
        }
    }
    mWake.notify_all();
    if (mThread.joinable()) {
        mThread.join();
    }
}

bool InputLink::handshake(int fd, std::string* error) {
    setTimeouts(fd, kOpenTimeoutSeconds);
    std::vector<uint8_t> frame;
    if (!writeAll(fd, withLength(encodeHello("dsa-audio"))) || !readFrame(fd, &frame)) {
        *error = "the host did not answer the hello";
        return false;
    }
    const HostMessage hello = decodeHostFrame(frame.data(), frame.size());
    if (const auto* rejected = std::get_if<Rejected>(&hello)) {
        *error = "the host refused the stream: " + rejected->detail;
        return false;
    }
    if (!std::holds_alternative<Accepted>(hello)) {
        *error = "the host answered the hello with something else";
        return false;
    }
    if (!writeAll(fd, withLength(encodeInputOpen(mName, mFramesPerBurst))) || !readFrame(fd, &frame)) {
        *error = "the host did not answer the open";
        return false;
    }
    const HostMessage answer = decodeHostFrame(frame.data(), frame.size());
    const auto* opened = std::get_if<InputOpened>(&answer);
    if (opened == nullptr) {
        *error = "the host answered the open with something else";
        return false;
    }
    if (!opened->error.empty()) {
        *error = opened->error;
        return false;
    }
    mHostDelay.store(opened->deviceFrames + static_cast<uint32_t>(kTargetFrames));
    // From here the thread waits for sound for as long as it takes; closing the link wakes it.
    setTimeouts(fd, 0);
    return true;
}

void InputLink::readInto(int fd) {
    std::vector<uint8_t> frame;
    for (;;) {
        if (!readFrame(fd, &frame)) {
            return;
        }
        const HostMessage message = decodeHostFrame(frame.data(), frame.size());
        if (const auto* pcm = std::get_if<Pcm>(&message)) {
            const auto* bytes = reinterpret_cast<const uint8_t*>(pcm->data.data());
            if (!mQueue.push(pcm->position, bytes, pcm->data.size())) {
                AUDIO_LOGW("the %s input got a piece that is not whole frames", mName.c_str());
            }
        }
    }
}

void InputLink::run(int fd) {
    UniqueFd socket(fd);
    for (;;) {
        readInto(socket.get());
        {
            std::lock_guard<std::mutex> lock(mLock);
            mSocket = -1;
            if (mStopping) return;
        }
        socket.reset();
        AUDIO_LOGW("the %s input lost the host; it is silent until the host records again",
                   mName.c_str());
        // A new host connection is a new timeline.
        mQueue.clear();
        for (;;) {
            {
                std::unique_lock<std::mutex> lock(mLock);
                mWake.wait_for(lock, kRetryDelay, [this] { return mStopping; });
                if (mStopping) return;
            }
            UniqueFd next(mConnect());
            std::string error;
            if (next.ok() && handshake(next.get(), &error)) {
                std::lock_guard<std::mutex> lock(mLock);
                if (mStopping) return;
                mSocket = next.get();
                socket = std::move(next);
                break;
            }
        }
        AUDIO_LOGI("the %s input records from the host again", mName.c_str());
    }
}

}  // namespace dsa::audio
