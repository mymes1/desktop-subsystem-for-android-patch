// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "stream_in.h"

#include <android/binder_ibinder.h>
#include <sys/resource.h>

#include <cstring>

#include "host_link.h"
#include "log.h"
#include "stream_support.h"

namespace dsa::audio {

using core::StreamDescriptor;
using namespace support;

namespace {

constexpr int32_t kFrameBytes = 4;

class QueueWriter final : public PcmWriter {
   public:
    explicit QueueWriter(SharedQueue& queue) : mQueue(queue) {}
    size_t availableToWrite() const override { return mQueue.availableToWrite(); }
    bool write(const uint8_t* data, size_t size) override { return mQueue.write(data, size); }

   private:
    SharedQueue& mQueue;
};

}  // namespace

StreamIn::StreamIn(int32_t portConfigId, int64_t bufferFrames, std::function<bool()> muted,
                   std::function<void(int32_t)> onClosed)
    : mPortConfigId(portConfigId),
      mBufferFrames(bufferFrames),
      mMuted(std::move(muted)),
      mOnClosed(std::move(onClosed)),
      mCommands(sizeof(StreamDescriptor::Command), sizeof(StreamDescriptor::Command), true),
      mReplies(sizeof(StreamDescriptor::Reply), sizeof(StreamDescriptor::Reply), true),
      mData(1, static_cast<size_t>(bufferFrames) * kFrameBytes, false) {}

std::shared_ptr<StreamIn> StreamIn::create(int32_t portConfigId, int64_t bufferFrames,
                                           std::function<bool()> muted,
                                           std::function<void(int32_t)> onClosed,
                                           StreamDescriptor* descriptor, std::string* error) {
    auto stream = ndk::SharedRefBase::make<StreamIn>(portConfigId, bufferFrames, std::move(muted),
                                                     std::move(onClosed));
    // It never opens, so the module is not told it closed: the module is opening it and
    // holds its own lock.
    if (!stream->mCommands.valid() || !stream->mReplies.valid() || !stream->mData.valid()) {
        *error = "cannot make the input stream's shared memory";
        stream->mOnClosed = nullptr;
        return nullptr;
    }
    stream->mHost = InputLink::open("primary", static_cast<uint32_t>(bufferFrames), connectToHost, error);
    if (stream->mHost == nullptr) {
        stream->mOnClosed = nullptr;
        return nullptr;
    }
    descriptor->command = describe<StreamDescriptor::Command>(stream->mCommands);
    descriptor->reply = describe<StreamDescriptor::Reply>(stream->mReplies);
    descriptor->frameSizeBytes = kFrameBytes;
    descriptor->bufferSizeFrames = bufferFrames;
    descriptor->audio.set<StreamDescriptor::AudioBuffer::Tag::fmq>(describe<int8_t>(stream->mData));
    std::weak_ptr<StreamIn> weak = stream;
    stream->mCommon = ndk::SharedRefBase::make<StreamCommon>([weak] {
        auto open = weak.lock();
        return open != nullptr && open->close();
    });
    stream->mWorker = std::thread([raw = stream.get()] { raw->work(); });
    return stream;
}

StreamIn::~StreamIn() {
    close();
}

bool StreamIn::close() {
    std::lock_guard<std::mutex> lock(mCloseLock);
    if (mClosed.exchange(true)) {
        return false;
    }
    mCommands.wake(kQueueNotEmpty);
    if (mWorker.joinable()) {
        mWorker.join();
    }
    // The host's capture device closes with the connection.
    mHost.reset();
    if (mOnClosed) {
        mOnClosed(mPortConfigId);
    }
    return true;
}

void StreamIn::work() {
    setpriority(PRIO_PROCESS, 0, kUrgentAudioPriority);
    MonotonicClock clock;
    QueueWriter writer(mData);
    StreamFormat format;
    format.bufferFrames = static_cast<uint32_t>(mBufferFrames);
    InputStreamLogic logic(*mHost, clock, format, mMuted);
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
        const StreamDescriptor::Reply reply = toReply(logic.handle(toCommand(command), writer));
        if (!mReplies.write(&reply, sizeof(reply))) {
            AUDIO_LOGE("the framework did not read the last reply; the input stream stops");
            break;
        }
        mReplies.wake(kQueueNotEmpty);
    }
}

ndk::ScopedAStatus StreamIn::getStreamCommon(std::shared_ptr<core::IStreamCommon>* _aidl_return) {
    *_aidl_return = mCommon;
    return ndk::ScopedAStatus::ok();
}

// The host's microphone is not described to the guest; the framework does without.
ndk::ScopedAStatus StreamIn::getActiveMicrophones(std::vector<media::MicrophoneDynamicInfo>* _aidl_return) {
    _aidl_return->clear();
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus StreamIn::getMicrophoneDirection(core::IStreamIn::MicrophoneDirection*) {
    return unsupported();
}
ndk::ScopedAStatus StreamIn::setMicrophoneDirection(core::IStreamIn::MicrophoneDirection) {
    return unsupported();
}
ndk::ScopedAStatus StreamIn::getMicrophoneFieldDimension(float*) { return unsupported(); }
ndk::ScopedAStatus StreamIn::setMicrophoneFieldDimension(float) { return unsupported(); }

ndk::ScopedAStatus StreamIn::updateMetadata(const common::SinkMetadata&) {
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus StreamIn::getHwGain(std::vector<float>*) { return unsupported(); }
ndk::ScopedAStatus StreamIn::setHwGain(const std::vector<float>&) { return unsupported(); }

}  // namespace dsa::audio
