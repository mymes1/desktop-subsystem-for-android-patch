// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// android.hardware.sensors.ISensors (version 2): the host's sensors as the guest's. The list
// is fixed for the life of the process, from the host's answer at start. Turning a sensor on
// or off, or changing its rate, is passed to the host; the host's readings are written into
// the framework's event queue as they come.

#pragma once

#include <sys/types.h>

#include <functional>
#include <memory>
#include <mutex>
#include <vector>

#include <aidl/android/hardware/sensors/BnSensors.h>

#include "event_queue.h"
#include "sensor_table.h"

namespace dsa::sensors {

class Sensors : public aidl::android::hardware::sensors::BnSensors {
   public:
    using Sender = std::function<bool(const std::vector<uint8_t>&)>;
    using SensorsCallback = aidl::android::hardware::sensors::ISensorsCallback;
    using EventDescriptor = aidl::android::hardware::common::fmq::MQDescriptor<
        aidl::android::hardware::sensors::Event, aidl::android::hardware::common::fmq::SynchronizedReadWrite>;
    using WakeLockDescriptor = aidl::android::hardware::common::fmq::MQDescriptor<
        int32_t, aidl::android::hardware::common::fmq::SynchronizedReadWrite>;

    Sensors(std::vector<SensorDef> sensors, Sender send);
    ~Sensors() override;

    // A batch of the host's readings, from the link's thread.
    void onReadings(const std::vector<Reading>& readings);

    // The host has connected (again): it is asked for every sensor that is on.
    void hostConnected();

    ndk::ScopedAStatus activate(int32_t sensorHandle, bool enabled) override;
    ndk::ScopedAStatus batch(int32_t sensorHandle, int64_t samplingPeriodNs, int64_t maxReportLatencyNs) override;
    ndk::ScopedAStatus configDirectReport(int32_t sensorHandle, int32_t channelHandle,
                                          aidl::android::hardware::sensors::ISensors::RateLevel rate,
                                          int32_t* result) override;
    ndk::ScopedAStatus flush(int32_t sensorHandle) override;
    ndk::ScopedAStatus getSensorsList(std::vector<aidl::android::hardware::sensors::SensorInfo>* result) override;
    ndk::ScopedAStatus initialize(const EventDescriptor& eventQueueDescriptor,
                                  const WakeLockDescriptor& wakeLockDescriptor,
                                  const std::shared_ptr<SensorsCallback>& sensorsCallback) override;
    ndk::ScopedAStatus injectSensorData(const aidl::android::hardware::sensors::Event& event) override;
    ndk::ScopedAStatus registerDirectChannel(const aidl::android::hardware::sensors::ISensors::SharedMemInfo& mem,
                                             int32_t* result) override;
    ndk::ScopedAStatus setOperationMode(aidl::android::hardware::sensors::ISensors::OperationMode mode) override;
    ndk::ScopedAStatus unregisterDirectChannel(int32_t channelHandle) override;

   private:
    struct Mapping {
        void* base;
        size_t length;
    };

    void ask(const Request& request);
    void unmapLocked();
    void writeLocked(const std::vector<aidl::android::hardware::sensors::Event>& events);

    const std::vector<SensorDef> mDefs;
    const Sender mSend;

    std::mutex mMutex;
    Activations mActivations;
    EventClock mClock;
    std::unique_ptr<EventQueue> mQueue;
    std::vector<Mapping> mMappings;
    std::shared_ptr<SensorsCallback> mCallback;
};

}  // namespace dsa::sensors
