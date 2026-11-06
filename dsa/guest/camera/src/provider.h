// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The guest's camera provider: the host computer's cameras, as Android's cameras. It asks the
// host for its cameras when it starts and every few seconds after, and tells the camera
// service when one comes or goes: plugged in or out, the host's window manager started or
// stopped, or the user's privacy settings changed on the host.

#pragma once

#include <aidl/android/hardware/camera/provider/BnCameraProvider.h>
#include <aidl/android/hardware/camera/provider/ICameraProviderCallback.h>

#include <condition_variable>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "camera_messages.h"
#include "stream_config.h"

namespace dsa::camera {

namespace provider = ::aidl::android::hardware::camera::provider;

// The provider's type, the first part of its instance and of each camera's name.
constexpr const char* kProviderType = "internal";

// The camera service's name for Android camera `id`: device@1.1/internal/<id>.
std::string deviceName(const std::string& id);
// The Android camera ID a device name stands for; empty when it is not one of this provider's.
std::string idOfDeviceName(const std::string& name);

class Provider : public provider::BnCameraProvider {
   public:
    Provider();
    ~Provider() override;

    // Lists the host's cameras now, then starts following them.
    void start();

    ndk::ScopedAStatus setCallback(const std::shared_ptr<provider::ICameraProviderCallback>& in_callback) override;
    ndk::ScopedAStatus getVendorTags(
        std::vector<::aidl::android::hardware::camera::common::VendorTagSection>* _aidl_return) override;
    ndk::ScopedAStatus getCameraIdList(std::vector<std::string>* _aidl_return) override;
    ndk::ScopedAStatus getCameraDeviceInterface(
        const std::string& in_cameraDeviceName,
        std::shared_ptr<::aidl::android::hardware::camera::device::ICameraDevice>* _aidl_return) override;
    ndk::ScopedAStatus notifyDeviceStateChange(int64_t in_deviceState) override;
    ndk::ScopedAStatus getConcurrentCameraIds(
        std::vector<provider::ConcurrentCameraIdCombination>* _aidl_return) override;
    ndk::ScopedAStatus isConcurrentStreamCombinationSupported(
        const std::vector<provider::CameraIdAndStreamCombination>& in_configs, bool* _aidl_return) override;

   private:
    void refresh();
    void follow();

    std::mutex mLock;
    std::condition_variable mStopping;
    bool mStopped = false;
    CameraIds mIds;
    // Android ID to host camera, for the cameras present now.
    std::map<std::string, HostCamera> mCameras;
    std::shared_ptr<provider::ICameraProviderCallback> mCallback;
    std::thread mFollower;
};

}  // namespace dsa::camera
