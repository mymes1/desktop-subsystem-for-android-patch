// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// One open camera: the streams an app configured, and its capture requests, each filled from
// the host camera's newest picture. The host camera runs from the first stream configuration
// until the session closes, so it is open only while an app uses it.

#pragma once

#include <aidl/android/hardware/camera/device/BnCameraDeviceSession.h>
#include <aidl/android/hardware/camera/device/ICameraDeviceCallback.h>
#include <android/hardware_buffer.h>

#include <condition_variable>
#include <cstdint>
#include <deque>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <utility>
#include <vector>

#include "camera_messages.h"
#include "host_link.h"
#include "metadata.h"

namespace dsa::camera {

namespace device = ::aidl::android::hardware::camera::device;
namespace fmq = ::aidl::android::hardware::common::fmq;

class Session : public device::BnCameraDeviceSession {
   public:
    Session(HostCamera camera, std::shared_ptr<device::ICameraDeviceCallback> callback);
    ~Session() override;

    bool closed();

    ndk::ScopedAStatus close() override;
    ndk::ScopedAStatus configureStreams(const device::StreamConfiguration& in_requestedConfiguration,
                                        std::vector<device::HalStream>* _aidl_return) override;
    ndk::ScopedAStatus constructDefaultRequestSettings(device::RequestTemplate in_type,
                                                       device::CameraMetadata* _aidl_return) override;
    ndk::ScopedAStatus flush() override;
    ndk::ScopedAStatus getCaptureRequestMetadataQueue(
        fmq::MQDescriptor<int8_t, fmq::SynchronizedReadWrite>* _aidl_return) override;
    ndk::ScopedAStatus getCaptureResultMetadataQueue(
        fmq::MQDescriptor<int8_t, fmq::SynchronizedReadWrite>* _aidl_return) override;
    ndk::ScopedAStatus isReconfigurationRequired(const device::CameraMetadata& in_oldSessionParams,
                                                 const device::CameraMetadata& in_newSessionParams,
                                                 bool* _aidl_return) override;
    ndk::ScopedAStatus processCaptureRequest(const std::vector<device::CaptureRequest>& in_requests,
                                             const std::vector<device::BufferCache>& in_cachesToRemove,
                                             int32_t* _aidl_return) override;
    ndk::ScopedAStatus signalStreamFlush(const std::vector<int32_t>& in_streamIds,
                                         int32_t in_streamConfigCounter) override;
    ndk::ScopedAStatus switchToOffline(const std::vector<int32_t>& in_streamsToKeep,
                                       device::CameraOfflineSessionInfo* out_offlineSessionInfo,
                                       std::shared_ptr<device::ICameraOfflineSession>* _aidl_return) override;
    ndk::ScopedAStatus repeatingRequestEnd(int32_t in_frameNumber, const std::vector<int32_t>& in_streamIds) override;

   private:
    struct StreamInfo {
        uint32_t width = 0;
        uint32_t height = 0;
        int32_t format = 0;
        uint64_t usage = 0;
        size_t bufferSize = 0;
    };
    struct Output {
        int32_t streamId = 0;
        int64_t bufferId = 0;
        AHardwareBuffer* buffer = nullptr;
        int acquireFence = -1;
    };
    struct Pending {
        int32_t frameNumber = 0;
        Metadata settings;
        std::vector<Output> outputs;
    };

    void work();
    void capture(Pending& pending);
    void fail(Pending& pending, bool notify);
    void deliver(device::CaptureResult result);
    // Starts the host camera at the configured size and `fps`; false says why in `error`.
    bool startHost(uint32_t fps, std::string* error);
    void releaseBuffers(const std::optional<int32_t>& streamId);

    const HostCamera mCamera;
    const std::shared_ptr<device::ICameraDeviceCallback> mCallback;

    std::mutex mLock;
    std::condition_variable mChanged;
    bool mClosed = false;
    bool mFlushing = false;
    int mInFlight = 0;
    std::deque<Pending> mQueue;
    std::map<int32_t, StreamInfo> mStreams;
    std::map<std::pair<int32_t, int64_t>, AHardwareBuffer*> mBuffers;
    Metadata mSettings;

    // Used by the worker, and by configuration while no request is pending.
    std::mutex mHostLock;
    std::unique_ptr<HostCameraLink> mHost;
    uint32_t mHostWidth = 0;
    uint32_t mHostHeight = 0;
    uint32_t mHostFps = 0;
    std::optional<uint64_t> mLastPicture;

    std::thread mWorker;
};

}  // namespace dsa::camera
