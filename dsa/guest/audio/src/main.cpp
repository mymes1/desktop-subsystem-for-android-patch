// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// dsa-audio: the guest's primary audio module, which plays the guest's sound on the host's
// default output device and records from its default capture device. It serves
// android.hardware.audio.core.IModule/default; the image's own audio service keeps the
// remote submix and Bluetooth modules, the configuration and the effects.

#include <android/binder_manager.h>
#include <android/binder_process.h>

#include <cstdlib>
#include <string>

#include "log.h"
#include "module.h"

int main() {
    // A stream's commands are handled on its own thread; binder calls only set it up.
    ABinderProcess_setThreadPoolMaxThreadCount(4);
    ABinderProcess_startThreadPool();

    auto module = ndk::SharedRefBase::make<dsa::audio::Module>();
    const std::string instance = std::string(dsa::audio::Module::descriptor) + "/default";
    const binder_status_t status = AServiceManager_addService(module->asBinder().get(), instance.c_str());
    if (status != STATUS_OK) {
        AUDIO_LOGE("cannot register %s (status %d)", instance.c_str(), status);
        return EXIT_FAILURE;
    }
    AUDIO_LOGI("serving %s; sound goes to the host's audio port", instance.c_str());
    ABinderProcess_joinThreadPool();
    return EXIT_FAILURE;
}
