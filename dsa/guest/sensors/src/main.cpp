// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// dsa-sensors: the guest's sensors HAL. It shows the host computer's accelerometer,
// gyroscope, orientation and light sensors to Android, each only when the computer has it.

#include <android/binder_manager.h>
#include <android/binder_process.h>

#include <chrono>
#include <csignal>
#include <memory>
#include <mutex>
#include <string>

#include "host_link.h"
#include "log.h"
#include "sensor_table.h"
#include "sensors_service.h"

using namespace dsa::sensors;

namespace {

// The framework asks for the sensor list once a boot, so the first list waits this long for
// the host. The host's window manager serves it and runs before the guest starts.
constexpr std::chrono::seconds kListTimeout{20};

}  // namespace

int main() {
    signal(SIGPIPE, SIG_IGN);

    std::mutex serviceLock;
    std::shared_ptr<Sensors> service;
    auto current = [&] {
        std::lock_guard<std::mutex> lock(serviceLock);
        return service;
    };
    HostLink link(
        [&](const std::vector<Reading>& readings) {
            if (const std::shared_ptr<Sensors> sensors = current()) {
                sensors->onReadings(readings);
            }
        },
        [&] {
            if (const std::shared_ptr<Sensors> sensors = current()) {
                sensors->hostConnected();
            }
        });
    link.start();

    const std::optional<std::vector<HostSensor>> host = link.waitForList(kListTimeout);
    if (!host) {
        SENSORS_LOGW("the host's sensor port did not answer in %lld s; Android has no sensors this boot",
                     static_cast<long long>(kListTimeout.count()));
    }
    const std::vector<SensorDef> sensors = sensorList(host.value_or(std::vector<HostSensor>{}));
    for (const SensorDef& def : sensors) {
        SENSORS_LOGI("sensor %d: %s (%s)", def.handle, def.name.c_str(), def.typeAsString.c_str());
    }
    {
        std::lock_guard<std::mutex> lock(serviceLock);
        service = ndk::SharedRefBase::make<Sensors>(
            sensors, [&link](const std::vector<uint8_t>& frame) { return link.send(frame); });
    }

    ABinderProcess_setThreadPoolMaxThreadCount(4);
    ABinderProcess_startThreadPool();

    const std::string instance = std::string(Sensors::descriptor) + "/default";
    const binder_status_t status = AServiceManager_addService(current()->asBinder().get(), instance.c_str());
    if (status != STATUS_OK) {
        SENSORS_LOGE("cannot register %s with the service manager (status %d)", instance.c_str(), status);
        return 1;
    }
    SENSORS_LOGI("serving %s with %zu of the host's sensors", instance.c_str(), sensors.size());

    ABinderProcess_joinThreadPool();
    return 1;
}
