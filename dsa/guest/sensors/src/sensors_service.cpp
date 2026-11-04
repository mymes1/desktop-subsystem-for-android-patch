// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "sensors_service.h"

#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#include <utility>

#include "log.h"
#include "sensor_messages.h"

namespace dsa::sensors {

using aidl::android::hardware::sensors::Event;
using aidl::android::hardware::sensors::ISensors;
using aidl::android::hardware::sensors::SensorInfo;
using aidl::android::hardware::sensors::SensorStatus;
using aidl::android::hardware::sensors::SensorType;
using EventPayload = Event::EventPayload;

namespace {

// libfmq's grantor order in a queue's descriptor.
constexpr size_t kReadCounter = 0;
constexpr size_t kWriteCounter = 1;
constexpr size_t kRing = 2;
constexpr size_t kFlagWord = 3;

int64_t bootTimeNs() {
    timespec now{};
    clock_gettime(CLOCK_BOOTTIME, &now);
    return static_cast<int64_t>(now.tv_sec) * 1'000'000'000 + now.tv_nsec;
}

Event toEvent(const PlainEvent& plain) {
    Event event;
    event.timestamp = plain.timestampNs;
    event.sensorHandle = plain.handle;
    event.sensorType = static_cast<SensorType>(plain.type);
    switch (plain.payload) {
        case Payload::Vec3: {
            EventPayload::Vec3 vec;
            vec.x = plain.values[0];
            vec.y = plain.values[1];
            vec.z = plain.values[2];
            vec.status = SensorStatus::ACCURACY_HIGH;
            event.payload.set<EventPayload::Tag::vec3>(vec);
            break;
        }
        case Payload::Data: {
            EventPayload::Data data;
            for (size_t i = 0; i < plain.values.size(); ++i) {
                data.values[i] = plain.values[i];
            }
            event.payload.set<EventPayload::Tag::data>(data);
            break;
        }
        case Payload::Scalar:
            event.payload.set<EventPayload::Tag::scalar>(plain.values[0]);
            break;
    }
    return event;
}

Event flushComplete(int32_t handle) {
    Event event;
    event.timestamp = 0;
    event.sensorHandle = handle;
    event.sensorType = SensorType::META_DATA;
    EventPayload::MetaData meta;
    meta.what = EventPayload::MetaData::MetaDataEventType::META_DATA_FLUSH_COMPLETE;
    event.payload.set<EventPayload::Tag::meta>(meta);
    return event;
}

SensorInfo toInfo(const SensorDef& def) {
    SensorInfo info;
    info.sensorHandle = def.handle;
    info.name = def.name;
    info.vendor = def.vendor;
    info.version = 1;
    info.type = static_cast<SensorType>(def.type);
    info.typeAsString = def.typeAsString;
    info.maxRange = def.maxRange;
    info.resolution = def.resolution;
    info.power = def.powerMa;
    info.minDelayUs = def.minDelayUs;
    info.fifoReservedEventCount = 0;
    info.fifoMaxEventCount = 0;
    info.requiredPermission = "";
    info.maxDelayUs = def.maxDelayUs;
    info.flags = def.flags;
    return info;
}

ndk::ScopedAStatus illegal() { return ndk::ScopedAStatus::fromExceptionCode(EX_ILLEGAL_ARGUMENT); }

ndk::ScopedAStatus unsupported() { return ndk::ScopedAStatus::fromExceptionCode(EX_UNSUPPORTED_OPERATION); }

}  // namespace

Sensors::Sensors(std::vector<SensorDef> sensors, Sender send)
    : mDefs(std::move(sensors)), mSend(std::move(send)), mActivations(mDefs) {}

Sensors::~Sensors() {
    std::lock_guard<std::mutex> lock(mMutex);
    unmapLocked();
}

void Sensors::ask(const Request& request) {
    if (!mSend(encodeActivate(request.kind, request.enabled, request.intervalUs))) {
        SENSORS_LOGW("the host is not connected; it is asked again when it connects");
    }
}

void Sensors::hostConnected() {
    std::vector<Request> on;
    {
        std::lock_guard<std::mutex> lock(mMutex);
        on = mActivations.active();
    }
    for (const Request& request : on) {
        ask(request);
    }
}

void Sensors::onReadings(const std::vector<Reading>& readings) {
    std::lock_guard<std::mutex> lock(mMutex);
    std::vector<Event> events;
    for (const PlainEvent& plain : mClock.stamp(readings, bootTimeNs())) {
        // A reading still on its way when the sensor was turned off is not the framework's.
        if (mActivations.enabled(plain.handle)) {
            events.push_back(toEvent(plain));
        }
    }
    writeLocked(events);
}

void Sensors::writeLocked(const std::vector<Event>& events) {
    if (events.empty() || !mQueue) {
        return;
    }
    const size_t written = mQueue->write(events.data(), events.size());
    if (written < events.size()) {
        SENSORS_LOGW("the framework's event queue is full; dropped %zu events", events.size() - written);
    }
    if (written > 0) {
        mQueue->wake(static_cast<uint32_t>(ISensors::EVENT_QUEUE_FLAG_BITS_READ_AND_PROCESS));
    }
}

ndk::ScopedAStatus Sensors::activate(int32_t sensorHandle, bool enabled) {
    std::optional<Request> request;
    {
        std::lock_guard<std::mutex> lock(mMutex);
        request = mActivations.activate(sensorHandle, enabled);
    }
    if (!request) {
        return illegal();
    }
    ask(*request);
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Sensors::batch(int32_t sensorHandle, int64_t samplingPeriodNs, int64_t /*maxReportLatencyNs*/) {
    std::optional<Request> request;
    {
        std::lock_guard<std::mutex> lock(mMutex);
        if (!mActivations.has(sensorHandle) || samplingPeriodNs < 0) {
            return illegal();
        }
        request = mActivations.batch(sensorHandle, samplingPeriodNs);
    }
    // The host sends what it has every few milliseconds; nothing is held back for later.
    if (request) {
        ask(*request);
    }
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Sensors::configDirectReport(int32_t, int32_t, ISensors::RateLevel, int32_t* result) {
    *result = ISensors::ERROR_BAD_VALUE;
    return unsupported();
}

ndk::ScopedAStatus Sensors::flush(int32_t sensorHandle) {
    std::lock_guard<std::mutex> lock(mMutex);
    if (!mActivations.has(sensorHandle)) {
        return illegal();
    }
    // Nothing is batched, so every event is already in the queue.
    writeLocked({flushComplete(sensorHandle)});
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Sensors::getSensorsList(std::vector<SensorInfo>* result) {
    result->clear();
    for (const SensorDef& def : mDefs) {
        result->push_back(toInfo(def));
    }
    return ndk::ScopedAStatus::ok();
}

void Sensors::unmapLocked() {
    mQueue.reset();
    for (const Mapping& mapping : mMappings) {
        munmap(mapping.base, mapping.length);
    }
    mMappings.clear();
}

ndk::ScopedAStatus Sensors::initialize(const EventDescriptor& eventQueueDescriptor,
                                       const WakeLockDescriptor& /*wakeLockDescriptor*/,
                                       const std::shared_ptr<SensorsCallback>& sensorsCallback) {
    std::vector<Request> off;
    {
        std::lock_guard<std::mutex> lock(mMutex);
        unmapLocked();
        // The framework starts (again) with every sensor off.
        off = mActivations.clear();
        mCallback = sensorsCallback;

        const EventDescriptor& desc = eventQueueDescriptor;
        if (desc.quantum != static_cast<int32_t>(sizeof(Event)) || desc.grantors.size() <= kRing) {
            SENSORS_LOGE("the framework's event queue has events of %d bytes and %zu parts; this HAL writes "
                         "events of %zu bytes into a queue of at least 3",
                         desc.quantum, desc.grantors.size(), sizeof(Event));
            return illegal();
        }
        const long page = sysconf(_SC_PAGESIZE);
        auto map = [&](size_t index) -> uint8_t* {
            if (index >= desc.grantors.size()) {
                return nullptr;
            }
            const auto& grantor = desc.grantors[index];
            if (grantor.fdIndex < 0 || static_cast<size_t>(grantor.fdIndex) >= desc.handle.fds.size() ||
                grantor.offset < 0 || grantor.extent <= 0 || page <= 0) {
                return nullptr;
            }
            const int fd = desc.handle.fds[static_cast<size_t>(grantor.fdIndex)].get();
            const off_t mapOffset = static_cast<off_t>((grantor.offset / page) * page);
            const size_t length = static_cast<size_t>(grantor.offset - mapOffset + grantor.extent);
            void* base = mmap(nullptr, length, PROT_READ | PROT_WRITE, MAP_SHARED, fd, mapOffset);
            if (base == MAP_FAILED) {
                return nullptr;
            }
            mMappings.push_back(Mapping{base, length});
            return static_cast<uint8_t*>(base) + (grantor.offset - mapOffset);
        };
        uint8_t* read = map(kReadCounter);
        uint8_t* write = map(kWriteCounter);
        uint8_t* ring = map(kRing);
        uint8_t* flag = map(kFlagWord);
        if (read == nullptr || write == nullptr || ring == nullptr) {
            SENSORS_LOGE("cannot map the framework's event queue");
            unmapLocked();
            return illegal();
        }
        mQueue = std::make_unique<EventQueue>(
            reinterpret_cast<std::atomic<uint64_t>*>(read), reinterpret_cast<std::atomic<uint64_t>*>(write), ring,
            static_cast<size_t>(desc.grantors[kRing].extent), sizeof(Event),
            reinterpret_cast<std::atomic<uint32_t>*>(flag));
        SENSORS_LOGI("the framework's event queue holds %zu events", mQueue->capacityItems());
    }
    for (const Request& request : off) {
        ask(request);
    }
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Sensors::injectSensorData(const Event&) { return unsupported(); }

ndk::ScopedAStatus Sensors::registerDirectChannel(const ISensors::SharedMemInfo&, int32_t* result) {
    *result = ISensors::ERROR_BAD_VALUE;
    return unsupported();
}

ndk::ScopedAStatus Sensors::setOperationMode(ISensors::OperationMode mode) {
    return mode == ISensors::OperationMode::NORMAL ? ndk::ScopedAStatus::ok() : unsupported();
}

ndk::ScopedAStatus Sensors::unregisterDirectChannel(int32_t) { return unsupported(); }

}  // namespace dsa::sensors
