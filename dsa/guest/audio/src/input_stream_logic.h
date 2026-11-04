// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// What the input stream does with each command the audio framework sends it, without the
// binder and the shared memory around it, so it runs in a test with fakes.
//
// The stream is synchronous, as the primary input is: a burst returns the frames it asked
// for once that much time has passed since the stream started, which is how the framework's
// record thread is paced. The pace comes from the guest's own monotonic clock, as for the
// output: a virtual capture head advances at the stream's rate while the stream runs, and a
// burst sleeps until the head has passed the frames it returns. What the host recorded fills
// those frames; what has not arrived is silence. The host is never waited on, so a host that
// stops sending leaves the stream running, silent. When the reader is late by more than a
// buffer, the frames it missed are counted as an overrun and the head restarts from what was
// read, so the next bursts are not handed out at once to catch up.
//
// The state machine is the one the audio HAL interface gives an input stream.

#pragma once

#include <cstddef>
#include <cstdint>
#include <functional>

#include "output_stream_logic.h"

namespace dsa::audio {

// Where the stream's sound comes from: the host's microphone, through the queue the link
// fills. Never waits.
class CaptureSource {
   public:
    virtual ~CaptureSource() = default;
    // Fills `out` with `frames` whole frames, silence for what is missing. Returns how many
    // were recorded sound.
    virtual size_t pull(uint8_t* out, size_t frames) = 0;
    // Drops what is queued, so the next start does not hand out stale sound.
    virtual void reset() = 0;
    // The capture device's own delay plus the host's, in frames; 0 when not known.
    virtual uint32_t hostDelayFrames() const = 0;
};

// The framework's side of the data queue, which the stream writes into.
class PcmWriter {
   public:
    virtual ~PcmWriter() = default;
    virtual size_t availableToWrite() const = 0;
    // Writes all of `size` bytes, or nothing. Returns whether it wrote.
    virtual bool write(const uint8_t* data, size_t size) = 0;
};

class InputStreamLogic {
   public:
    // `muted` says whether the framework muted the microphone (IModule::setMicMute); the
    // stream then gives silence in time, as a muted microphone does.
    InputStreamLogic(CaptureSource& source, Clock& clock, StreamFormat format,
                     std::function<bool()> muted);

    // Handles one command, writing a burst's sound into `writer`.
    Reply handle(const Command& command, PcmWriter& writer);

    StreamState state() const { return mState; }
    // Frames handed to the framework since the stream opened.
    uint64_t framesRead() const { return mRead; }
    int32_t latencyMs() const;

   private:
    Reply reply(int32_t status) const;
    int64_t captureHead(int64_t now) const;
    void startClock(int64_t now);
    void stopClock();
    bool burst(int32_t bytes, PcmWriter& writer, Reply* out);

    CaptureSource& mSource;
    Clock& mClock;
    StreamFormat mFormat;
    std::function<bool()> mMuted;
    StreamState mState = StreamState::Standby;
    uint64_t mRead = 0;
    bool mRunning = false;
    int64_t mAnchorNs = 0;
    uint64_t mAnchorFrames = 0;
    int64_t mXrunFrames = 0;
};

}  // namespace dsa::audio
