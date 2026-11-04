// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "output_stream_logic.h"

#include <algorithm>
#include <cstdint>
#include <limits>
#include <vector>

namespace dsa::audio {

namespace {

constexpr int64_t kNsPerSecond = 1000000000;
// What the host adds when it has not said: its 40 ms queue and about 10 ms in the device.
constexpr uint32_t kDefaultHostDelayMs = 50;

}  // namespace

OutputStreamLogic::OutputStreamLogic(PcmSink& sink, Clock& clock, StreamFormat format)
    : mSink(sink), mClock(clock), mFormat(format) {}

int32_t OutputStreamLogic::latencyMs() const {
    const uint32_t host = mSink.hostDelayFrames();
    const uint64_t hostMs =
            host == 0 ? kDefaultHostDelayMs : static_cast<uint64_t>(host) * 1000 / mFormat.sampleRate;
    const uint64_t ownMs = static_cast<uint64_t>(mFormat.bufferFrames) * 1000 / mFormat.sampleRate;
    return static_cast<int32_t>(ownMs + hostMs);
}

int64_t OutputStreamLogic::playHead(int64_t now) const {
    if (!mRunning) {
        return static_cast<int64_t>(mStoppedAt);
    }
    const int64_t elapsed = std::max<int64_t>(0, now - mAnchorNs);
    return static_cast<int64_t>(mAnchorFrames) + elapsed * mFormat.sampleRate / kNsPerSecond;
}

void OutputStreamLogic::startClock(int64_t now) {
    if (mRunning) return;
    mRunning = true;
    mAnchorNs = now;
    mAnchorFrames = std::min<uint64_t>(mStoppedAt, mWritten);
}

void OutputStreamLogic::stopClock(int64_t now) {
    if (!mRunning) return;
    mStoppedAt = static_cast<uint64_t>(std::min<int64_t>(playHead(now), static_cast<int64_t>(mWritten)));
    mRunning = false;
}

Reply OutputStreamLogic::reply(int32_t status) const {
    Reply r;
    r.status = status;
    r.state = mState;
    r.latencyMs = latencyMs();
    r.xrunFrames = static_cast<int32_t>(std::min<int64_t>(mXrunFrames, std::numeric_limits<int32_t>::max()));
    const int64_t now = mClock.nowNs();
    uint32_t delay = mSink.hostDelayFrames();
    if (delay == 0) {
        delay = kDefaultHostDelayMs * mFormat.sampleRate / 1000;
    }
    // What has been heard: the play head, less what the host still holds back. It never goes
    // back, and never passes what was written.
    int64_t presented = playHead(now) - static_cast<int64_t>(delay);
    presented = std::clamp<int64_t>(presented, 0, static_cast<int64_t>(mWritten));
    presented = std::max(presented, mLastPresented);
    mLastPresented = presented;
    r.observableFrames = presented;
    r.observableTimeNs = now;
    r.hardwareFrames = presented;
    r.hardwareTimeNs = now;
    return r;
}

bool OutputStreamLogic::burst(int32_t bytes, PcmSource& source, Reply* out) {
    const int64_t now = mClock.nowNs();
    const bool active = mState == StreamState::Active;
    if (active) {
        startClock(now);
        const int64_t head = playHead(now);
        if (head > static_cast<int64_t>(mWritten)) {
            // The mixer came late: those frames were never sent, and the host played silence.
            mXrunFrames += head - static_cast<int64_t>(mWritten);
            mAnchorNs = now;
            mAnchorFrames = mWritten;
        }
    }
    // Whole frames only; a stray byte at the end stays for the next burst.
    const size_t wanted = static_cast<size_t>(bytes) / mFormat.frameBytes * mFormat.frameBytes;
    std::vector<uint8_t> data(wanted);
    size_t got = 0;
    while (got < wanted) {
        const size_t n = source.read(data.data() + got, wanted - got);
        if (n == 0) break;
        got += n;
    }
    got = got / mFormat.frameBytes * mFormat.frameBytes;
    if (got > 0) {
        mSink.pcm(mWritten, data.data(), got);
        mWritten += got / mFormat.frameBytes;
    }
    if (active && mWritten > mAnchorFrames + mFormat.bufferFrames) {
        const uint64_t ahead = mWritten - mFormat.bufferFrames - mAnchorFrames;
        const int64_t deadline =
                mAnchorNs + static_cast<int64_t>(ahead * kNsPerSecond / mFormat.sampleRate);
        if (deadline > now) {
            mClock.sleepUntilNs(deadline);
        }
    }
    out->fmqByteCount = static_cast<int32_t>(got);
    return true;
}

Reply OutputStreamLogic::handle(const Command& command, PcmSource& source) {
    const int64_t now = mClock.nowNs();
    switch (command.kind) {
        case Command::Kind::Exit:
        case Command::Kind::GetStatus:
            return reply(kStatusOk);

        case Command::Kind::Start:
            if (mState == StreamState::Standby) {
                mState = StreamState::Idle;
                return reply(kStatusOk);
            }
            if (mState == StreamState::Paused) {
                mState = StreamState::Active;
                startClock(now);
                mSink.command(HostCommand::Start);
                mHostPlaying = true;
                return reply(kStatusOk);
            }
            return reply(kStatusInvalidOperation);

        case Command::Kind::Burst: {
            if (command.bytes < 0) {
                return reply(kStatusBadValue);
            }
            switch (mState) {
                case StreamState::Standby:
                case StreamState::Paused:
                    // Sound before a start primes the host's queue, which holds it.
                    mState = StreamState::Paused;
                    break;
                case StreamState::Idle:
                case StreamState::Active:
                    mState = StreamState::Active;
                    if (!mHostPlaying) {
                        mSink.command(HostCommand::Start);
                        mHostPlaying = true;
                    }
                    break;
                default:
                    return reply(kStatusInvalidOperation);
            }
            Reply r;
            burst(command.bytes, source, &r);
            Reply full = reply(kStatusOk);
            full.fmqByteCount = r.fmqByteCount;
            return full;
        }

        case Command::Kind::Drain:
            if (mState != StreamState::Active) {
                return reply(kStatusInvalidOperation);
            }
            // The host plays out what it holds on its own; the stream is idle at once.
            stopClock(now);
            mStoppedAt = mWritten;
            mState = StreamState::Idle;
            return reply(kStatusOk);

        case Command::Kind::Standby:
            if (mState != StreamState::Idle && mState != StreamState::Paused) {
                return reply(kStatusInvalidOperation);
            }
            stopClock(now);
            mStoppedAt = mWritten;
            mState = StreamState::Standby;
            mSink.command(HostCommand::Standby);
            mHostPlaying = false;
            return reply(kStatusOk);

        case Command::Kind::Pause:
            if (mState != StreamState::Active) {
                return reply(kStatusInvalidOperation);
            }
            stopClock(now);
            mState = StreamState::Paused;
            mSink.command(HostCommand::Pause);
            mHostPlaying = false;
            return reply(kStatusOk);

        case Command::Kind::Flush:
            if (mState != StreamState::Paused) {
                return reply(kStatusInvalidOperation);
            }
            mStoppedAt = mWritten;
            mState = StreamState::Idle;
            mSink.command(HostCommand::Flush);
            return reply(kStatusOk);

        case Command::Kind::Unsupported:
            return reply(kStatusInvalidOperation);
    }
    return reply(kStatusBadValue);
}

}  // namespace dsa::audio
