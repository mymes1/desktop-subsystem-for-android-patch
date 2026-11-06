// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// dsa-gnss: the guest's GNSS HAL. While the framework wants locations, the host's location
// service runs, and each position it finds becomes the guest's GNSS location.

#include <android/binder_manager.h>
#include <android/binder_process.h>

#include <csignal>
#include <memory>
#include <string>

#include "channel_client.h"
#include "gnss_service.h"
#include "log.h"

using namespace dsa::gnss;

int main() {
    signal(SIGPIPE, SIG_IGN);

    std::shared_ptr<Gnss> gnss;
    ChannelClient channel(
        [&gnss](const std::vector<uint8_t>& frame) {
            if (gnss) {
                gnss->hostFrame(frame);
            }
        },
        [&gnss] {
            if (gnss) {
                gnss->channelConnected();
            }
        });
    gnss = ndk::SharedRefBase::make<Gnss>(
        [&channel](const std::vector<uint8_t>& frame) { return channel.send(frame); });

    ABinderProcess_setThreadPoolMaxThreadCount(2);
    ABinderProcess_startThreadPool();

    const std::string instance = std::string(Gnss::descriptor) + "/default";
    const binder_status_t status = AServiceManager_addService(gnss->asBinder().get(), instance.c_str());
    if (status != STATUS_OK) {
        GNSS_LOGE("cannot register %s with the service manager (status %d)", instance.c_str(), status);
        return 1;
    }
    GNSS_LOGI("serving %s; locations come from the host's location service", instance.c_str());

    channel.start();
    ABinderProcess_joinThreadPool();
    return 1;
}
