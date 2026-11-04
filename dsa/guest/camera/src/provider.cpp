// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "provider.h"

#include <aidl/android/hardware/camera/common/CameraDeviceStatus.h>
#include <aidl/android/hardware/camera/common/Status.h>

#include <chrono>

#include "device.h"
#include "host_link.h"
#include "log.h"

namespace dsa::camera {

namespace common = ::aidl::android::hardware::camera::common;

namespace {

// How often the provider asks the host for its cameras again.
constexpr std::chrono::seconds kFollowEvery{3};

const std::string kNamePrefix = std::string("device@1.1/") + kProviderType + "/";

bool sameCamera(const HostCamera& a, const HostCamera& b) {
    if (a.id != b.id || a.facing != b.facing || a.sensorOrientation != b.sensorOrientation ||
        a.modes.size() != b.modes.size()) {
        return false;
    }
    for (size_t i = 0; i < a.modes.size(); ++i) {
        if (a.modes[i].width != b.modes[i].width || a.modes[i].height != b.modes[i].height ||
            a.modes[i].maxFps != b.modes[i].maxFps || a.modes[i].minFps != b.modes[i].minFps) {
            return false;
        }
    }
    return true;
}

}  // namespace

std::string deviceName(const std::string& id) { return kNamePrefix + id; }

std::string idOfDeviceName(const std::string& name) {
    if (name.compare(0, kNamePrefix.size(), kNamePrefix) != 0) {
        return std::string();
    }
    return name.substr(kNamePrefix.size());
}

Provider::Provider() = default;

Provider::~Provider() {
    {
        std::lock_guard<std::mutex> lock(mLock);
        mStopped = true;
    }
    mStopping.notify_all();
    if (mFollower.joinable()) {
        mFollower.join();
    }
}

void Provider::start() {
    refresh();
    mFollower = std::thread([this] { follow(); });
}

void Provider::follow() {
    std::unique_lock<std::mutex> lock(mLock);
    while (!mStopping.wait_for(lock, kFollowEvery, [this] { return mStopped; })) {
        lock.unlock();
        refresh();
        lock.lock();
    }
}

void Provider::refresh() {
    std::string error;
    // The host not answering means no camera: its window manager is not running, so nobody
    // is there to lend one.
    std::vector<HostCamera> listed = listHostCameras(&error).value_or(std::vector<HostCamera>());

    std::vector<std::pair<std::string, common::CameraDeviceStatus>> changes;
    std::shared_ptr<provider::ICameraProviderCallback> callback;
    {
        std::lock_guard<std::mutex> lock(mLock);
        std::map<std::string, HostCamera> present;
        for (HostCamera& camera : listed) {
            const std::string id = mIds.idFor(camera.id);
            present.emplace(id, std::move(camera));
        }
        for (const auto& [id, camera] : mCameras) {
            auto now = present.find(id);
            if (now == present.end()) {
                CAMERA_LOGI("camera %s (%s) is gone", id.c_str(), camera.name.c_str());
                changes.emplace_back(id, common::CameraDeviceStatus::NOT_PRESENT);
            } else if (!sameCamera(camera, now->second)) {
                // Its modes changed: it goes and comes back, so the service reads it again.
                changes.emplace_back(id, common::CameraDeviceStatus::NOT_PRESENT);
                changes.emplace_back(id, common::CameraDeviceStatus::PRESENT);
            }
        }
        for (const auto& [id, camera] : present) {
            if (mCameras.count(id) == 0) {
                CAMERA_LOGI("camera %s is %s, facing %d, turned %u degrees", id.c_str(), camera.name.c_str(),
                            static_cast<int>(camera.facing), camera.sensorOrientation);
                changes.emplace_back(id, common::CameraDeviceStatus::PRESENT);
            }
        }
        mCameras = std::move(present);
        callback = mCallback;
    }
    if (callback == nullptr) {
        return;
    }
    for (const auto& [id, status] : changes) {
        const ndk::ScopedAStatus sent = callback->cameraDeviceStatusChange(deviceName(id), status);
        if (!sent.isOk()) {
            CAMERA_LOGW("the camera service did not hear of camera %s: %s", id.c_str(),
                        sent.getDescription().c_str());
        }
    }
}

ndk::ScopedAStatus Provider::setCallback(const std::shared_ptr<provider::ICameraProviderCallback>& in_callback) {
    if (in_callback == nullptr) {
        return ndk::ScopedAStatus::fromServiceSpecificError(static_cast<int32_t>(common::Status::ILLEGAL_ARGUMENT));
    }
    std::lock_guard<std::mutex> lock(mLock);
    mCallback = in_callback;
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Provider::getVendorTags(std::vector<common::VendorTagSection>* _aidl_return) {
    _aidl_return->clear();
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Provider::getCameraIdList(std::vector<std::string>* _aidl_return) {
    std::lock_guard<std::mutex> lock(mLock);
    _aidl_return->clear();
    for (const auto& [id, camera] : mCameras) {
        _aidl_return->push_back(deviceName(id));
    }
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Provider::getCameraDeviceInterface(
    const std::string& in_cameraDeviceName,
    std::shared_ptr<::aidl::android::hardware::camera::device::ICameraDevice>* _aidl_return) {
    const std::string id = idOfDeviceName(in_cameraDeviceName);
    std::lock_guard<std::mutex> lock(mLock);
    auto camera = mCameras.find(id);
    if (id.empty() || camera == mCameras.end()) {
        return ndk::ScopedAStatus::fromServiceSpecificError(static_cast<int32_t>(common::Status::ILLEGAL_ARGUMENT));
    }
    *_aidl_return = ndk::SharedRefBase::make<Device>(camera->second);
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Provider::notifyDeviceStateChange(int64_t) {
    // A computer does not fold, and covering its camera changes nothing here.
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Provider::getConcurrentCameraIds(std::vector<provider::ConcurrentCameraIdCombination>* _aidl_return) {
    _aidl_return->clear();
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Provider::isConcurrentStreamCombinationSupported(
    const std::vector<provider::CameraIdAndStreamCombination>&, bool* _aidl_return) {
    *_aidl_return = false;
    return ndk::ScopedAStatus::ok();
}

}  // namespace dsa::camera
