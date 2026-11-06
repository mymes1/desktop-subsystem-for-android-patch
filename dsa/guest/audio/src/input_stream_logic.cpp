// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "input_stream_logic.h"

#include <algorithm>
#include <cstring>
#include <limits>
#include <vector>

namespace dsa::audio {

namespace {

constexpr int64_t kNsPerSecond = 1000000000;
// What the host adds when it has not said: about 10 ms in the device and the guest's 20 ms
// queue.
constexpr uint32_t kDefaultHostDelayMs = 30;

}  // namespace

InputStreamLogic::InputStreamLogic(CaptureSource& source, Clock& clock, StreamFormat format,
                                   std::function<bool()> muted)
    : mSource(source), mClock(clock), mFormat(format), mMuted(std::move(muted)) {}

int32_t InputStreamLogic::latencyMs() const {
    const uint32_t host = mSource.hostDelayFrames();
    const uint64_t hostMs =
            host == 0 ? kDefaultHostDelayMs : static_cast<uint64_t>(host) * 1000 / mFormat.sampleRate;
    const uint64_t ownMs = static_cast<uint64_t>(mFormat.bufferFrames) * 1000 / mFormat.sampleRate;
    return static_cast<int32_t>(ownMs + hostMs);
}

int64_t InputStreamLogic::captureHead(int64_t now) const {
    if (!mRunning) {
        return static_cast<int64_t>(mRead);
    }
    const int64_t elapsed = std::max<int64_t>(0, now - mAnchorNs);
    return static_cast<int64_t>(mAnchorFrames) + elapsed * mFormat.sampleRate / kNsPerSecond;
}

void InputStreamLogic::startClock(int64_t now) {
    if (mRunning) return;
    mRunning = true;
    mAnchorNs = now;
    mAnchorFrames = mRead;
}

void InputStreamLogic::stopClock() { mRunning = false; }

Reply InputStreamLogic::reply(int32_t status) const {
    Reply r;
    r.status = status;
    r.state = mState;
    r.latencyMs = latencyMs();
    r.xrunFrames = static_cast<int32_t>(std::min<int64_t>(mXrunFrames, std::numeric_limits<int32_t>::max()));
    const int64_t now = mClock.nowNs();
    // For an input, the position is the frames read; the newest of them was captured now.
    r.observableFrames = static_cast<int64_t>(mRead);
    r.observableTimeNs = now;
    r.hardwareFrames = static_cast<int64_t>(mRead);
    r.hardwareTimeNs = now;
    return r;
}

bool InputStreamLogic::burst(int32_t bytes, PcmWriter& writer, Reply* out) {
    const size_t room = std::min(static_cast<size_t>(bytes), writer.availableToWrite());
    const size_t frames = room / mFormat.frameBytes;
    out->fmqByteCount = 0;
    if (frames == 0) {
        return true;
    }
    int64_t now = mClock.nowNs();
    startClock(now);
    const int64_t head = captureHead(now);
    if (head > static_cast<int64_t>(mRead + mFormat.bufferFrames)) {
        // The reader came late: what was captured meanwhile is gone, as on a real device.
        mXrunFrames += head - static_cast<int64_t>(mRead);
        mAnchorNs = now;
        mAnchorFrames = mRead;
    }
    // Wait until the frames this burst returns have been captured.
    const uint64_t until = mRead + frames;
    const int64_t deadline =
            mAnchorNs + static_cast<int64_t>((until - mAnchorFrames) * kNsPerSecond / mFormat.sampleRate);
    if (deadline > now) {
        mClock.sleepUntilNs(deadline);
    }
    std::vector<uint8_t> data(frames * mFormat.frameBytes);
    mSource.pull(data.data(), frames);
    if (mMuted && mMuted()) {
        std::memset(data.data(), 0, data.size());
    }
    if (!writer.write(data.data(), data.size())) {
        return false;
    }
    mRead += frames;
    out->fmqByteCount = static_cast<int32_t>(data.size());
    return true;
}

Reply InputStreamLogic::handle(const Command& command, PcmWriter& writer) {
    switch (command.kind) {
        case Command::Kind::Exit:
        case Command::Kind::GetStatus:
            return reply(kStatusOk);

        case Command::Kind::Start:
            if (mState == StreamState::Standby) {
                // Sound queued while the stream stood by is old; start from what comes now.
                mSource.reset();
                mState = StreamState::Idle;
                return reply(kStatusOk);
            }
            if (mState == StreamState::Draining) {
                mState = StreamState::Active;
                return reply(kStatusOk);
            }
            return reply(kStatusInvalidOperation);

        case Command::Kind::Burst: {
            if (command.bytes < 0) {
                return reply(kStatusBadValue);
            }
            if (mState != StreamState::Idle && mState != StreamState::Active &&
                mState != StreamState::Paused && mState != StreamState::Draining) {
                return reply(kStatusInvalidOperation);
            }
            Reply r;
            const bool wrote = burst(command.bytes, writer, &r);
            if (mState == StreamState::Idle || mState == StreamState::Paused) {
                mState = StreamState::Active;
            } else if (mState == StreamState::Draining) {
                // Nothing more is held back: the drain ends with this burst.
                stopClock();
                mState = StreamState::Standby;
            }
            Reply full = reply(wrote ? kStatusOk : kStatusNotEnoughData);
            full.fmqByteCount = r.fmqByteCount;
            return full;
        }

        case Command::Kind::Drain:
            if (mState != StreamState::Active) {
                return reply(kStatusInvalidOperation);
            }
            mState = StreamState::Draining;
            return reply(kStatusOk);

        case Command::Kind::Standby:
            if (mState != StreamState::Idle) {
                return reply(kStatusInvalidOperation);
            }
            stopClock();
            mState = StreamState::Standby;
            return reply(kStatusOk);

        case Command::Kind::Pause:
            if (mState != StreamState::Active) {
                return reply(kStatusInvalidOperation);
            }
            stopClock();
            mState = StreamState::Paused;
            return reply(kStatusOk);

        case Command::Kind::Flush:
            if (mState != StreamState::Paused) {
                return reply(kStatusInvalidOperation);
            }
            mSource.reset();
            mState = StreamState::Standby;
            return reply(kStatusOk);

        case Command::Kind::Unsupported:
            return reply(kStatusInvalidOperation);
    }
    return reply(kStatusBadValue);
}

}  // namespace dsa::audio
