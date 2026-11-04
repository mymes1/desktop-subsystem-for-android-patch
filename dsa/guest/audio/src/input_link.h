// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// One input stream's connection to the host's audio port (vsock 5568). Opening it asks the
// host to open its default capture device, and fails with the host's reason when the host
// lets no program record (its privacy setting) or has no microphone, so the framework's open
// of the stream fails and the app is told. While the stream is open, a thread of the link's
// own reads what the host records into a CaptureQueue, which the stream reads at its own
// pace. When the host goes away (the window manager restarts), the stream goes on silent and
// the link opens the host's device again once it answers. Closing the link closes the
// connection, and with it the host's device.

#pragma once

#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <functional>
#include <memory>
#include <mutex>
#include <string>
#include <thread>

#include "capture_queue.h"
#include "input_stream_logic.h"
#include "unique_fd.h"

namespace dsa::audio {

class InputLink final : public CaptureSource {
   public:
    // How long an open waits for the host's answer, so a stuck host never holds up the
    // framework's open for long.
    static constexpr int kOpenTimeoutSeconds = 2;

    // Connects with `connect` (a connected socket, or -1), says Hello and opens the stream.
    // Returns null, with the reason in `error`, when the host does not record.
    static std::unique_ptr<InputLink> open(std::string name, uint32_t framesPerBurst,
                                           std::function<int()> connect, std::string* error);
    ~InputLink() override;

    size_t pull(uint8_t* out, size_t frames) override { return mQueue.pull(out, frames); }
    void reset() override { mQueue.clear(); }
    uint32_t hostDelayFrames() const override { return mHostDelay.load(); }

    CaptureStats stats() const { return mQueue.stats(); }

    InputLink(std::string name, uint32_t framesPerBurst, std::function<int()> connect);

   private:
    // Opens the stream on `fd`. On the host's refusal or no answer, returns false with the
    // reason in `error`.
    bool handshake(int fd, std::string* error);
    void run(int fd);
    // Reads what the host records until the connection ends.
    void readInto(int fd);

    const std::string mName;
    const uint32_t mFramesPerBurst;
    std::function<int()> mConnect;
    CaptureQueue mQueue;
    std::mutex mLock;
    std::condition_variable mWake;
    bool mStopping = false;
    // The socket the thread reads, so closing the link can wake it.
    int mSocket = -1;
    std::thread mThread;
    std::atomic<uint32_t> mHostDelay{0};
};

}  // namespace dsa::audio
