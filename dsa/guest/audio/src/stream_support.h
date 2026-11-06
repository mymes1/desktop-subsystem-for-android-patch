// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// What the output and the input stream share around their logic: the guest's clock, the
// descriptors of their shared queues, and the conversion of the framework's commands and
// replies.

#pragma once

#include <aidl/android/hardware/audio/core/StreamDescriptor.h>
#include <time.h>
#include <unistd.h>

#include <cerrno>
#include <cstdint>

#include "output_stream_logic.h"
#include "shared_queue.h"

namespace dsa::audio {
namespace support {

using ::aidl::android::hardware::audio::core::StreamDescriptor;
using ::aidl::android::hardware::common::fmq::GrantorDescriptor;
using ::aidl::android::hardware::common::fmq::MQDescriptor;
using ::aidl::android::hardware::common::fmq::SynchronizedReadWrite;

// How long a stream's thread waits for a command before it checks whether it was closed.
constexpr int64_t kCommandWaitNs = 100 * 1000 * 1000;
// The platform's ANDROID_PRIORITY_URGENT_AUDIO.
constexpr int kUrgentAudioPriority = -19;

class MonotonicClock final : public Clock {
   public:
    int64_t nowNs() override {
        timespec now{};
        clock_gettime(CLOCK_MONOTONIC, &now);
        return static_cast<int64_t>(now.tv_sec) * 1000000000 + now.tv_nsec;
    }
    void sleepUntilNs(int64_t deadline) override {
        timespec until{};
        until.tv_sec = static_cast<time_t>(deadline / 1000000000);
        until.tv_nsec = static_cast<long>(deadline % 1000000000);
        while (clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &until, nullptr) == EINTR) {
        }
    }
};

template <typename T>
inline MQDescriptor<T, SynchronizedReadWrite> describe(const SharedQueue& queue) {
    MQDescriptor<T, SynchronizedReadWrite> descriptor;
    for (int i = 0; i < queue.grantorCount(); ++i) {
        const Grantor g = queue.grantor(i);
        GrantorDescriptor grantor;
        grantor.fdIndex = g.fdIndex;
        grantor.offset = g.offset;
        grantor.extent = g.extent;
        descriptor.grantors.push_back(grantor);
    }
    descriptor.handle.fds.emplace_back(dup(queue.fd()));
    descriptor.quantum = queue.quantum();
    descriptor.flags = kSynchronizedReadWrite;
    return descriptor;
}

inline Command toCommand(const StreamDescriptor::Command& command) {
    using Tag = StreamDescriptor::Command::Tag;
    Command c;
    switch (command.getTag()) {
        case Tag::halReservedExit:
            c.kind = Command::Kind::Exit;
            break;
        case Tag::getStatus:
            c.kind = Command::Kind::GetStatus;
            break;
        case Tag::start:
            c.kind = Command::Kind::Start;
            break;
        case Tag::burst:
            c.kind = Command::Kind::Burst;
            c.bytes = command.get<Tag::burst>();
            break;
        case Tag::drain:
            c.kind = Command::Kind::Drain;
            break;
        case Tag::standby:
            c.kind = Command::Kind::Standby;
            break;
        case Tag::pause:
            c.kind = Command::Kind::Pause;
            break;
        case Tag::flush:
            c.kind = Command::Kind::Flush;
            break;
        case Tag::flushFromFrame:
            // The module tells the framework it cannot flush from a frame.
            c.kind = Command::Kind::Unsupported;
            break;
    }
    return c;
}

inline StreamDescriptor::Reply toReply(const Reply& r) {
    StreamDescriptor::Reply reply;
    reply.status = r.status;
    reply.fmqByteCount = r.fmqByteCount;
    reply.observable.frames = r.observableFrames;
    reply.observable.timeNs = r.observableTimeNs;
    reply.hardware.frames = r.hardwareFrames;
    reply.hardware.timeNs = r.hardwareTimeNs;
    reply.latencyMs = r.latencyMs;
    reply.xrunFrames = r.xrunFrames;
    reply.state = static_cast<StreamDescriptor::State>(static_cast<int32_t>(r.state));
    return reply;
}

inline ndk::ScopedAStatus unsupported() {
    return ndk::ScopedAStatus::fromExceptionCode(EX_UNSUPPORTED_OPERATION);
}

}  // namespace support
}  // namespace dsa::audio