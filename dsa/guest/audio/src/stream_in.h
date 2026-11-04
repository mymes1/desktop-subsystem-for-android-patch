// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The primary input stream, the host's microphone: the HAL interface object the framework
// holds, the shared queues it talks to it through, and the thread that runs each command it
// sends (InputStreamLogic) with the sound the host records (InputLink).
//
// The stream opens only when the host records: creating it opens the host's capture device
// and fails with the host's reason when it cannot, so the framework's open fails and the app
// is told. Closing it closes the host's device. The app's own permission to record is the
// framework's, checked before any stream opens.

#pragma once

#include <aidl/android/hardware/audio/core/BnStreamIn.h>
#include <aidl/android/hardware/audio/core/StreamDescriptor.h>

#include <atomic>
#include <functional>
#include <memory>
#include <mutex>
#include <string>
#include <thread>

#include "input_link.h"
#include "input_stream_logic.h"
#include "shared_queue.h"
#include "stream_out.h"

namespace dsa::audio {

class StreamIn final : public core::BnStreamIn {
   public:
    // A stream of `bufferFrames` frames at a time, whose descriptor goes in `descriptor`.
    // `muted` says whether the framework muted the microphone. `onClosed` runs once, when the
    // framework closes it. Returns null, with the reason in `error`, when the host does not
    // record or the shared memory cannot be made.
    static std::shared_ptr<StreamIn> create(int32_t portConfigId, int64_t bufferFrames,
                                            std::function<bool()> muted,
                                            std::function<void(int32_t)> onClosed,
                                            core::StreamDescriptor* descriptor, std::string* error);
    ~StreamIn() override;

    // Stops the stream's thread and its connection to the host. Returns false if it was
    // closed already.
    bool close();

    ndk::ScopedAStatus getStreamCommon(std::shared_ptr<core::IStreamCommon>* _aidl_return) override;
    ndk::ScopedAStatus getActiveMicrophones(
            std::vector<media::MicrophoneDynamicInfo>* _aidl_return) override;
    ndk::ScopedAStatus getMicrophoneDirection(core::IStreamIn::MicrophoneDirection* _aidl_return) override;
    ndk::ScopedAStatus setMicrophoneDirection(core::IStreamIn::MicrophoneDirection in_direction) override;
    ndk::ScopedAStatus getMicrophoneFieldDimension(float* _aidl_return) override;
    ndk::ScopedAStatus setMicrophoneFieldDimension(float in_zoom) override;
    ndk::ScopedAStatus updateMetadata(const common::SinkMetadata& in_sinkMetadata) override;
    ndk::ScopedAStatus getHwGain(std::vector<float>* _aidl_return) override;
    ndk::ScopedAStatus setHwGain(const std::vector<float>& in_channelGains) override;

    StreamIn(int32_t portConfigId, int64_t bufferFrames, std::function<bool()> muted,
             std::function<void(int32_t)> onClosed);

   private:
    void work();

    const int32_t mPortConfigId;
    const int64_t mBufferFrames;
    std::function<bool()> mMuted;
    std::function<void(int32_t)> mOnClosed;
    SharedQueue mCommands;
    SharedQueue mReplies;
    SharedQueue mData;
    std::unique_ptr<InputLink> mHost;
    std::shared_ptr<StreamCommon> mCommon;
    std::atomic<bool> mClosed{false};
    std::mutex mCloseLock;
    std::thread mWorker;
};

}  // namespace dsa::audio
