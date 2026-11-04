// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// dsa-camera: the guest's camera provider HAL. The host computer's cameras become Android's
// cameras; cameraserver reaches them through it, so Android's camera permission applies to
// every app as for any camera.

#include <android/binder_manager.h>
#include <android/binder_process.h>

#include <csignal>
#include <string>

#include "log.h"
#include "provider.h"

int main() {
    // A host connection that drops must not kill the process.
    signal(SIGPIPE, SIG_IGN);

    ABinderProcess_setThreadPoolMaxThreadCount(6);
    ABinderProcess_startThreadPool();

    auto provider = ndk::SharedRefBase::make<dsa::camera::Provider>();
    provider->start();
    const std::string instance =
        std::string(dsa::camera::Provider::descriptor) + "/" + dsa::camera::kProviderType + "/0";
    const binder_status_t status = AServiceManager_addService(provider->asBinder().get(), instance.c_str());
    if (status != STATUS_OK) {
        CAMERA_LOGE("cannot register %s with the service manager (status %d)", instance.c_str(), status);
        return 1;
    }
    CAMERA_LOGI("serving %s; the host computer's cameras are Android's", instance.c_str());
    ABinderProcess_joinThreadPool();
    return 1;
}
