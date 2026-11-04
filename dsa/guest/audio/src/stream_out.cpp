// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "stream_out.h"

#include <android/binder_ibinder.h>
#include <sys/resource.h>
#include <time.h>
#include <unistd.h>

#include <cstring>

#include "log.h"

namespace dsa::audio {

using core::StreamDescriptor;
using ::aidl::android::hardware::common::fmq::GrantorDescriptor;
using ::aidl::android::hardware::common::fmq::MQDescriptor;
using ::aidl::android::hardware::common::fmq::SynchronizedReadWrite;

namespace {

constexpr int32_t kFrameBytes = 4;
// How long the stream's thread waits for a command before it checks whether it was closed.
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

class QueueSource final : public PcmSource {
   public:
    explicit QueueSource(SharedQueue& queue) : mQueue(queue) {}
    size_t read(uint8_t* out, size_t max) override { return mQueue.read(out, max); }

   private:
    SharedQueue& mQueue;
};

template <typename T>
MQDescriptor<T, SynchronizedReadWrite> describe(const SharedQueue& queue) {
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

Command toCommand(const StreamDescriptor::Command& command) {
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

StreamDescriptor::Reply toReply(const Reply& r) {
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

ndk::ScopedAStatus unsupported() {
    return ndk::ScopedAStatus::fromExceptionCode(EX_UNSUPPORTED_OPERATION);
}

}  // namespace

StreamOut::StreamOut(int32_t portConfigId, int64_t bufferFrames,
                     std::function<void(int32_t)> onClosed)
    : mPortConfigId(portConfigId),
      mBufferFrames(bufferFrames),
      mOnClosed(std::move(onClosed)),
      mCommands(sizeof(StreamDescriptor::Command), sizeof(StreamDescriptor::Command), true),
      mReplies(sizeof(StreamDescriptor::Reply), sizeof(StreamDescriptor::Reply), true),
      mData(1, static_cast<size_t>(bufferFrames) * kFrameBytes, false) {}

std::shared_ptr<StreamOut> StreamOut::create(int32_t portConfigId, int64_t bufferFrames,
                                             std::function<void(int32_t)> onClosed,
                                             StreamDescriptor* descriptor) {
    auto stream = ndk::SharedRefBase::make<StreamOut>(portConfigId, bufferFrames, std::move(onClosed));
    if (!stream->mCommands.valid() || !stream->mReplies.valid() || !stream->mData.valid()) {
        AUDIO_LOGE("cannot make the output stream's shared memory");
        // It never opened, so the module is not told it closed: the module is opening it
        // and holds its own lock.
        stream->mOnClosed = nullptr;
        return nullptr;
    }
    descriptor->command = describe<StreamDescriptor::Command>(stream->mCommands);
    descriptor->reply = describe<StreamDescriptor::Reply>(stream->mReplies);
    descriptor->frameSizeBytes = kFrameBytes;
    descriptor->bufferSizeFrames = bufferFrames;
    descriptor->audio.set<StreamDescriptor::AudioBuffer::Tag::fmq>(describe<int8_t>(stream->mData));
    stream->mCommon = ndk::SharedRefBase::make<StreamCommon>(stream);
    stream->mHost = std::make_unique<HostLink>("primary", static_cast<uint32_t>(bufferFrames),
                                               connectToHost);
    stream->mWorker = std::thread([raw = stream.get()] { raw->work(); });
    return stream;
}

StreamOut::~StreamOut() {
    close();
}

bool StreamOut::close() {
    std::lock_guard<std::mutex> lock(mCloseLock);
    if (mClosed.exchange(true)) {
        return false;
    }
    mCommands.wake(kQueueNotEmpty);
    if (mWorker.joinable()) {
        mWorker.join();
    }
    mHost.reset();
    if (mOnClosed) {
        mOnClosed(mPortConfigId);
    }
    return true;
}

void StreamOut::work() {
    setpriority(PRIO_PROCESS, 0, kUrgentAudioPriority);
    MonotonicClock clock;
    QueueSource source(mData);
    StreamFormat format;
    format.bufferFrames = static_cast<uint32_t>(mBufferFrames);
    OutputStreamLogic logic(*mHost, clock, format);
    while (!mClosed.load()) {
        if (mCommands.availableToRead() < sizeof(StreamDescriptor::Command)) {
            mCommands.wait(kQueueNotEmpty, kCommandWaitNs);
            continue;
        }
        StreamDescriptor::Command command;
        uint8_t raw[sizeof(StreamDescriptor::Command)];
        mCommands.read(raw, sizeof(raw));
        std::memcpy(static_cast<void*>(&command), raw, sizeof(raw));
        mCommands.wake(kQueueNotFull);
        const StreamDescriptor::Reply reply = toReply(logic.handle(toCommand(command), source));
        if (!mReplies.write(&reply, sizeof(reply))) {
            AUDIO_LOGE("the framework did not read the last reply; the output stream stops");
            break;
        }
        mReplies.wake(kQueueNotEmpty);
    }
}

ndk::ScopedAStatus StreamOut::getStreamCommon(std::shared_ptr<core::IStreamCommon>* _aidl_return) {
    *_aidl_return = mCommon;
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus StreamOut::updateMetadata(const common::SourceMetadata&) {
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus StreamOut::updateOffloadMetadata(const common::AudioOffloadMetadata&) {
    return ndk::ScopedAStatus::fromExceptionCode(EX_ILLEGAL_STATE);
}

ndk::ScopedAStatus StreamOut::getHwVolume(std::vector<float>*) { return unsupported(); }
ndk::ScopedAStatus StreamOut::setHwVolume(const std::vector<float>&) { return unsupported(); }
ndk::ScopedAStatus StreamOut::getAudioDescriptionMixLevel(float*) { return unsupported(); }
ndk::ScopedAStatus StreamOut::setAudioDescriptionMixLevel(float) { return unsupported(); }
ndk::ScopedAStatus StreamOut::getDualMonoMode(media::AudioDualMonoMode*) { return unsupported(); }
ndk::ScopedAStatus StreamOut::setDualMonoMode(media::AudioDualMonoMode) { return unsupported(); }
ndk::ScopedAStatus StreamOut::getRecommendedLatencyModes(std::vector<media::AudioLatencyMode>*) {
    return unsupported();
}
ndk::ScopedAStatus StreamOut::setLatencyMode(media::AudioLatencyMode) { return unsupported(); }
ndk::ScopedAStatus StreamOut::getPlaybackRateParameters(media::AudioPlaybackRate*) {
    return unsupported();
}
ndk::ScopedAStatus StreamOut::setPlaybackRateParameters(const media::AudioPlaybackRate&) {
    return unsupported();
}
ndk::ScopedAStatus StreamOut::selectPresentation(int32_t, int32_t) { return unsupported(); }

ndk::ScopedAStatus StreamCommon::close() {
    auto stream = mStream.lock();
    if (stream == nullptr || !stream->close()) {
        return ndk::ScopedAStatus::fromExceptionCode(EX_ILLEGAL_STATE);
    }
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus StreamCommon::prepareToClose() { return ndk::ScopedAStatus::ok(); }

ndk::ScopedAStatus StreamCommon::updateHwAvSyncId(int32_t) { return unsupported(); }

ndk::ScopedAStatus StreamCommon::getVendorParameters(const std::vector<std::string>&,
                                                     std::vector<core::VendorParameter>*) {
    return ndk::ScopedAStatus::fromExceptionCode(EX_ILLEGAL_ARGUMENT);
}

ndk::ScopedAStatus StreamCommon::setVendorParameters(const std::vector<core::VendorParameter>&, bool) {
    return ndk::ScopedAStatus::fromExceptionCode(EX_ILLEGAL_ARGUMENT);
}

ndk::ScopedAStatus StreamCommon::addEffect(const std::shared_ptr<effect::IEffect>&) {
    return unsupported();
}

ndk::ScopedAStatus StreamCommon::removeEffect(const std::shared_ptr<effect::IEffect>&) {
    return unsupported();
}

ndk::ScopedAStatus StreamCommon::createMmapBuffer(core::MmapBufferDescriptor*) {
    // The output is not a shared memory (MMAP) stream.
    return unsupported();
}

}  // namespace dsa::audio
