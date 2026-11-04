// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "device.h"

#include <aidl/android/hardware/camera/common/Status.h>

#include "characteristics.h"
#include "log.h"
#include "stream_config.h"

namespace dsa::camera {

namespace common = ::aidl::android::hardware::camera::common;

namespace {

ndk::ScopedAStatus error(common::Status status) {
    return ndk::ScopedAStatus::fromServiceSpecificError(static_cast<int32_t>(status));
}

}  // namespace

Device::Device(HostCamera camera) : mCamera(std::move(camera)) {}

ndk::ScopedAStatus Device::getCameraCharacteristics(device::CameraMetadata* _aidl_return) {
    _aidl_return->metadata = characteristics(mCamera).bytes();
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Device::getPhysicalCameraCharacteristics(const std::string&, device::CameraMetadata*) {
    // A host camera is one camera, not a logical camera of several.
    return error(common::Status::ILLEGAL_ARGUMENT);
}

ndk::ScopedAStatus Device::getResourceCost(common::CameraResourceCost* _aidl_return) {
    // Each host camera is a device of its own on the host; none takes another's place.
    _aidl_return->resourceCost = 100;
    _aidl_return->conflictingDevices.clear();
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Device::isStreamCombinationSupported(const device::StreamConfiguration& in_streams,
                                                        bool* _aidl_return) {
    std::vector<StreamAsk> asks;
    for (const device::Stream& stream : in_streams.streams) {
        asks.push_back(StreamAsk{static_cast<int32_t>(stream.format), static_cast<uint32_t>(stream.width),
                                 static_cast<uint32_t>(stream.height),
                                 stream.streamType == device::StreamType::OUTPUT});
    }
    std::string why;
    *_aidl_return = streamsSupported(mCamera, asks, &why);
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Device::open(const std::shared_ptr<device::ICameraDeviceCallback>& in_callback,
                                std::shared_ptr<device::ICameraDeviceSession>* _aidl_return) {
    if (in_callback == nullptr) {
        return error(common::Status::ILLEGAL_ARGUMENT);
    }
    std::lock_guard<std::mutex> lock(mLock);
    if (auto running = mSession.lock(); running && !running->closed()) {
        return error(common::Status::CAMERA_IN_USE);
    }
    auto session = ndk::SharedRefBase::make<Session>(mCamera, in_callback);
    mSession = session;
    *_aidl_return = session;
    CAMERA_LOGI("%s opened", mCamera.name.c_str());
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Device::openInjectionSession(const std::shared_ptr<device::ICameraDeviceCallback>&,
                                                std::shared_ptr<device::ICameraInjectionSession>*) {
    return error(common::Status::OPERATION_NOT_SUPPORTED);
}

ndk::ScopedAStatus Device::setTorchMode(bool) {
    // A computer's camera has no flash.
    return error(common::Status::OPERATION_NOT_SUPPORTED);
}

ndk::ScopedAStatus Device::turnOnTorchWithStrengthLevel(int32_t) {
    return error(common::Status::OPERATION_NOT_SUPPORTED);
}

ndk::ScopedAStatus Device::getTorchStrengthLevel(int32_t*) {
    return error(common::Status::OPERATION_NOT_SUPPORTED);
}

}  // namespace dsa::camera
