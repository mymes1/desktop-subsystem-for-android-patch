// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// One host camera, as Android's camera service sees a camera device: its characteristics,
// and one session at a time.

#pragma once

#include <aidl/android/hardware/camera/device/BnCameraDevice.h>

#include <memory>
#include <mutex>

#include "camera_messages.h"
#include "session.h"

namespace dsa::camera {

class Device : public device::BnCameraDevice {
   public:
    explicit Device(HostCamera camera);

    ndk::ScopedAStatus getCameraCharacteristics(device::CameraMetadata* _aidl_return) override;
    ndk::ScopedAStatus getPhysicalCameraCharacteristics(const std::string& in_physicalCameraId,
                                                        device::CameraMetadata* _aidl_return) override;
    ndk::ScopedAStatus getResourceCost(::aidl::android::hardware::camera::common::CameraResourceCost* _aidl_return) override;
    ndk::ScopedAStatus isStreamCombinationSupported(const device::StreamConfiguration& in_streams,
                                                    bool* _aidl_return) override;
    ndk::ScopedAStatus open(const std::shared_ptr<device::ICameraDeviceCallback>& in_callback,
                            std::shared_ptr<device::ICameraDeviceSession>* _aidl_return) override;
    ndk::ScopedAStatus openInjectionSession(const std::shared_ptr<device::ICameraDeviceCallback>& in_callback,
                                            std::shared_ptr<device::ICameraInjectionSession>* _aidl_return) override;
    ndk::ScopedAStatus setTorchMode(bool in_on) override;
    ndk::ScopedAStatus turnOnTorchWithStrengthLevel(int32_t in_torchStrength) override;
    ndk::ScopedAStatus getTorchStrengthLevel(int32_t* _aidl_return) override;

   private:
    const HostCamera mCamera;
    std::mutex mLock;
    std::weak_ptr<Session> mSession;
};

}  // namespace dsa::camera
