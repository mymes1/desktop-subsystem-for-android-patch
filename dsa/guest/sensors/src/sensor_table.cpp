// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "sensor_table.h"

#include <algorithm>

namespace dsa::sensors {

namespace {

struct KindInfo {
    int32_t type;
    const char* typeAsString;
    int32_t flags;
    size_t valueCount;
    Payload payload;
};

KindInfo infoOf(Kind kind) {
    switch (kind) {
        case Kind::Accelerometer:
            return {kTypeAccelerometer, "android.sensor.accelerometer", kFlagContinuous, 3, Payload::Vec3};
        case Kind::Gyroscope:
            return {kTypeGyroscope, "android.sensor.gyroscope", kFlagContinuous, 3, Payload::Vec3};
        case Kind::RotationVector:
            return {kTypeRotationVector, "android.sensor.rotation_vector", kFlagContinuous, 4, Payload::Data};
        case Kind::Light:
            return {kTypeLight, "android.sensor.light", kFlagOnChange, 1, Payload::Scalar};
    }
    return {kTypeMetaData, "", kFlagContinuous, 0, Payload::Scalar};
}

uint32_t clampInterval(int64_t periodNs, int32_t minDelayUs) {
    int64_t us = periodNs / 1000;
    us = std::max<int64_t>(us, minDelayUs);
    us = std::min<int64_t>(us, kMaxDelayUs);
    return static_cast<uint32_t>(std::max<int64_t>(us, 0));
}

}  // namespace

int32_t handleOf(Kind kind) { return static_cast<int32_t>(kind); }

std::optional<Kind> kindOfHandle(int32_t handle) {
    if (handle < static_cast<int32_t>(Kind::Accelerometer) || handle > static_cast<int32_t>(Kind::Light)) {
        return std::nullopt;
    }
    return static_cast<Kind>(handle);
}

std::vector<SensorDef> sensorList(const std::vector<HostSensor>& host) {
    std::vector<SensorDef> list;
    for (const HostSensor& sensor : host) {
        const int32_t handle = handleOf(sensor.kind);
        const bool seen = std::any_of(list.begin(), list.end(), [&](const SensorDef& d) { return d.handle == handle; });
        if (seen || !kindOfHandle(handle)) {
            continue;
        }
        const KindInfo info = infoOf(sensor.kind);
        SensorDef def;
        def.handle = handle;
        def.type = info.type;
        def.name = sensor.name.empty() ? std::string("DSA ") + info.typeAsString : sensor.name;
        def.vendor = sensor.vendor.empty() ? "DSA" : sensor.vendor;
        def.typeAsString = info.typeAsString;
        def.maxRange = sensor.maxRange;
        def.resolution = sensor.resolution;
        // The host's sensor draws the host's power, not the guest's.
        def.powerMa = 0.001f;
        const int32_t minDelayUs =
            static_cast<int32_t>(std::min<uint32_t>(sensor.minIntervalUs, static_cast<uint32_t>(kMaxDelayUs)));
        def.minDelayUs = minDelayUs;
        def.maxDelayUs = kMaxDelayUs;
        def.flags = info.flags;
        list.push_back(std::move(def));
    }
    std::sort(list.begin(), list.end(), [](const SensorDef& a, const SensorDef& b) { return a.handle < b.handle; });
    return list;
}

Activations::Activations(const std::vector<SensorDef>& sensors) {
    for (const SensorDef& def : sensors) {
        const std::optional<Kind> kind = kindOfHandle(def.handle);
        if (!kind) {
            continue;
        }
        State state{*kind};
        state.minDelayUs = def.minDelayUs;
        mSensors.emplace(def.handle, state);
    }
}

Request Activations::request(const State& state) const {
    return Request{state.kind, state.enabled, state.enabled ? clampInterval(state.periodNs, state.minDelayUs) : 0};
}

bool Activations::has(int32_t handle) const { return mSensors.count(handle) != 0; }

bool Activations::enabled(int32_t handle) const {
    auto found = mSensors.find(handle);
    return found != mSensors.end() && found->second.enabled;
}

std::optional<Request> Activations::activate(int32_t handle, bool enabled) {
    auto found = mSensors.find(handle);
    if (found == mSensors.end()) {
        return std::nullopt;
    }
    found->second.enabled = enabled;
    return request(found->second);
}

std::optional<Request> Activations::batch(int32_t handle, int64_t samplingPeriodNs) {
    auto found = mSensors.find(handle);
    if (found == mSensors.end()) {
        return std::nullopt;
    }
    found->second.periodNs = samplingPeriodNs > 0 ? samplingPeriodNs : kDefaultPeriodNs;
    if (!found->second.enabled) {
        return std::nullopt;
    }
    return request(found->second);
}

std::vector<Request> Activations::active() const {
    std::vector<Request> on;
    for (const auto& [handle, state] : mSensors) {
        if (state.enabled) {
            on.push_back(request(state));
        }
    }
    return on;
}

std::vector<Request> Activations::clear() {
    std::vector<Request> off;
    for (auto& [handle, state] : mSensors) {
        if (state.enabled) {
            state.enabled = false;
            off.push_back(request(state));
        }
        state.periodNs = kDefaultPeriodNs;
    }
    return off;
}

std::vector<PlainEvent> EventClock::stamp(const std::vector<Reading>& readings, int64_t nowNs) {
    std::vector<PlainEvent> events;
    if (readings.empty()) {
        return events;
    }
    uint64_t newestUs = 0;
    for (const Reading& reading : readings) {
        newestUs = std::max(newestUs, reading.timestampUs);
    }
    for (const Reading& reading : readings) {
        const KindInfo info = infoOf(reading.kind);
        if (info.valueCount == 0 || reading.values.size() < info.valueCount) {
            continue;
        }
        PlainEvent event;
        event.handle = handleOf(reading.kind);
        event.type = info.type;
        event.payload = info.payload;
        const int64_t ageNs = static_cast<int64_t>(newestUs - reading.timestampUs) * 1000;
        int64_t timestamp = nowNs - ageNs;
        auto last = mLast.find(event.handle);
        if (last != mLast.end() && timestamp <= last->second) {
            timestamp = last->second + 1;
        }
        mLast[event.handle] = timestamp;
        event.timestampNs = timestamp;
        for (size_t i = 0; i < info.valueCount; ++i) {
            event.values[i] = reading.values[i];
        }
        if (reading.kind == Kind::RotationVector) {
            // The heading's accuracy: not known.
            event.values[4] = -1.0f;
        }
        events.push_back(event);
    }
    return events;
}

}  // namespace dsa::sensors
