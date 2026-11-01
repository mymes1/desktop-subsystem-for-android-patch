// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// What the output stream does with each command the audio framework sends it, without the
// binder and the shared memory around it, so it runs in a test with fakes.
//
// The stream is synchronous, as the primary output is: a burst returns once the sound it
// carried may be handed to the host without the host's queue growing, which is how the
// framework's mixer thread is paced. The pace comes from the guest's own monotonic clock: the
// stream keeps a virtual play head that advances at the stream's rate while it plays, and a
// burst sleeps until the play head is no more than one buffer behind what was written. The
// host is never waited on. When the mixer is late (its own underrun), the play head is ahead
// of what was written; the frames it skipped are counted as an underrun and the play head
// restarts from what was written, so the next bursts are not rushed out to catch up. What the
// host does not hear in time it plays as silence.

#pragma once

#include <cstddef>
#include <cstdint>

namespace dsa::audio {

// The stream's states, with the values the HAL interface gives them.
enum class StreamState : int32_t {
    Standby = 1,
    Idle = 2,
    Active = 3,
    Paused = 4,
    Draining = 5,
    DrainPaused = 6,
    Transferring = 7,
    TransferPaused = 8,
    Error = 100,
};

// The reply statuses the HAL interface defines.
constexpr int32_t kStatusOk = 0;
constexpr int32_t kStatusBadValue = -22;          // -EINVAL
constexpr int32_t kStatusInvalidOperation = -38;  // -ENOSYS
constexpr int32_t kStatusNotEnoughData = -61;     // -ENODATA

constexpr int64_t kPositionUnknown = -1;

struct Command {
    enum class Kind { Exit, GetStatus, Start, Burst, Drain, Standby, Pause, Flush };
    Kind kind = Kind::GetStatus;
    // For Burst: the bytes the framework put in the data queue.
    int32_t bytes = 0;
};

struct Reply {
    int32_t status = kStatusOk;
    int32_t fmqByteCount = 0;
    int64_t observableFrames = kPositionUnknown;
    int64_t observableTimeNs = kPositionUnknown;
    int64_t hardwareFrames = kPositionUnknown;
    int64_t hardwareTimeNs = kPositionUnknown;
    int32_t latencyMs = 0;
    int32_t xrunFrames = 0;
    StreamState state = StreamState::Standby;
};

// What the host is asked to do with the stream.
enum class HostCommand { Start, Pause, Flush, Standby };

// Where the stream's sound goes: the connection to the host. Neither call may wait for the
// host; what cannot be sent at once is dropped.
class PcmSink {
   public:
    virtual ~PcmSink() = default;
    // Whole frames of 48 kHz stereo 16-bit sound, `position` frames into the stream.
    virtual void pcm(uint64_t position, const uint8_t* data, size_t size) = 0;
    virtual void command(HostCommand command) = 0;
    // The frames the host queues before it plays, and the device's own delay, as the host
    // last said. 0 when not known.
    virtual uint32_t hostDelayFrames() const = 0;
};

// The framework's side of the data queue.
class PcmSource {
   public:
    virtual ~PcmSource() = default;
    virtual size_t read(uint8_t* out, size_t max) = 0;
};

class Clock {
   public:
    virtual ~Clock() = default;
    // CLOCK_MONOTONIC, in nanoseconds.
    virtual int64_t nowNs() = 0;
    virtual void sleepUntilNs(int64_t deadline) = 0;
};

struct StreamFormat {
    uint32_t sampleRate = 48000;
    uint32_t frameBytes = 4;
    // The frames the framework writes at a time, which is also how far the play head may
    // trail what was written.
    uint32_t bufferFrames = 960;
};

class OutputStreamLogic {
   public:
    OutputStreamLogic(PcmSink& sink, Clock& clock, StreamFormat format);

    // Handles one command, reading a burst's sound from `source`.
    Reply handle(const Command& command, PcmSource& source);

    StreamState state() const { return mState; }
    // Frames written since the stream opened.
    uint64_t framesWritten() const { return mWritten; }
    // The delay from a frame being written to it being heard, in milliseconds.
    int32_t latencyMs() const;

   private:
    Reply reply(int32_t status) const;
    int64_t playHead(int64_t now) const;
    void startClock(int64_t now);
    void stopClock(int64_t now);
    bool burst(int32_t bytes, PcmSource& source, Reply* out);

    PcmSink& mSink;
    Clock& mClock;
    StreamFormat mFormat;
    StreamState mState = StreamState::Standby;
    uint64_t mWritten = 0;
    // While the clock runs: the play head was at mAnchorFrames at mAnchorNs.
    bool mRunning = false;
    int64_t mAnchorNs = 0;
    uint64_t mAnchorFrames = 0;
    // The play head while the clock is stopped.
    uint64_t mStoppedAt = 0;
    // The last position reported, so positions never go back.
    mutable int64_t mLastPresented = 0;
    int64_t mXrunFrames = 0;
    bool mHostPlaying = false;
};

}  // namespace dsa::audio
