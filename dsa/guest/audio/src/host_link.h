// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// One output stream's connection to the host's audio port (vsock 5568). The stream's thread
// only ever queues and sends without waiting; a thread of the link's own connects, does the
// handshake and opens the stream, and connects again after the host goes away (the window
// manager restarts, the user signs out and in). While there is no connection the sound is
// dropped: the guest plays on, silent on the host, and never waits for it.

#pragma once

#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <functional>
#include <mutex>
#include <string>
#include <thread>

#include "outbound.h"
#include "output_stream_logic.h"
#include "unique_fd.h"

namespace dsa::audio {

// The vsock port of the host's audio streams.
constexpr uint32_t kAudioPort = 5568;

class HostLink final : public PcmSink {
   public:
    // Opens connections with `connect`, which returns a connected socket or -1 and may take a
    // while; it runs on the link's own thread. `framesPerBurst` goes in the stream's format.
    HostLink(std::string name, uint32_t framesPerBurst, std::function<int()> connect);
    ~HostLink() override;

    void pcm(uint64_t position, const uint8_t* data, size_t size) override;
    void command(HostCommand command) override;
    uint32_t hostDelayFrames() const override { return mHostDelay.load(); }

    // Frames dropped because the host was not there or not reading.
    uint64_t droppedFrames() const { return mDropped.load(); }

   private:
    void run();
    bool open(int fd);
    void lost();
    void sendLocked(const std::vector<uint8_t>& frame, bool isPcm, size_t frames);

    const std::string mName;
    const uint32_t mFramesPerBurst;
    std::function<int()> mConnect;
    std::thread mThread;
    std::mutex mLock;
    std::condition_variable mWake;
    bool mStopping = false;
    UniqueFd mSocket;
    Outbound mOutbound;
    // The last state command, sent again on a new connection so the host's stream matches.
    bool mHasState = false;
    HostCommand mState = HostCommand::Standby;
    std::atomic<uint32_t> mHostDelay{0};
    std::atomic<uint64_t> mDropped{0};
};

// Connects to the host's audio port. Returns the socket, or -1.
int connectToHost();

}  // namespace dsa::audio
