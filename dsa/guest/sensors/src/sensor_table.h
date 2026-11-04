// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The sensors HAL's rules, apart from binder: which Android sensors the host's sensors
// become, what the host is asked for as the framework turns them on and off, and the events
// each reading becomes. Plain types only, so they build and are tested on any machine.

#pragma once

#include <array>
#include <cstdint>
#include <map>
#include <optional>
#include <string>
#include <vector>

#include "sensor_messages.h"

namespace dsa::sensors {

// android.hardware.sensors.SensorType values this HAL reports.
constexpr int32_t kTypeMetaData = 0;
constexpr int32_t kTypeAccelerometer = 1;
constexpr int32_t kTypeGyroscope = 4;
constexpr int32_t kTypeLight = 5;
constexpr int32_t kTypeRotationVector = 11;

// SensorInfo flags: how a sensor reports.
constexpr int32_t kFlagContinuous = 0;
constexpr int32_t kFlagOnChange = 2;

// The rate a sensor runs at until the framework says otherwise: SENSOR_DELAY_NORMAL.
constexpr int64_t kDefaultPeriodNs = 200'000'000;

// The longest time between readings the host gives.
constexpr int32_t kMaxDelayUs = 1'000'000;

// One Android sensor, as getSensorsList describes it.
struct SensorDef {
    int32_t handle = 0;
    int32_t type = 0;
    std::string name;
    std::string vendor;
    std::string typeAsString;
    float maxRange = 0;
    float resolution = 0;
    float powerMa = 0;
    int32_t minDelayUs = 0;
    int32_t maxDelayUs = 0;
    int32_t flags = 0;
};

// Each kind has a handle of its own, the same on every boot: its protocol number.
int32_t handleOf(Kind kind);
std::optional<Kind> kindOfHandle(int32_t handle);

// The Android sensors for the host's, one per kind, in handle order. A kind the host does not
// have is not there.
std::vector<SensorDef> sensorList(const std::vector<HostSensor>& host);

// What the host is asked for.
struct Request {
    Kind kind = Kind::Accelerometer;
    bool enabled = false;
    uint32_t intervalUs = 0;

    bool operator==(const Request& other) const {
        return kind == other.kind && enabled == other.enabled && intervalUs == other.intervalUs;
    }
};

// Which sensors the framework has on, and at what rate.
class Activations {
   public:
    explicit Activations(const std::vector<SensorDef>& sensors);

    // The framework turns a sensor on or off. Nothing for a handle this HAL does not have.
    std::optional<Request> activate(int32_t handle, bool enabled);

    // The framework sets a sensor's rate. Asks the host only while the sensor is on.
    std::optional<Request> batch(int32_t handle, int64_t samplingPeriodNs);

    // Every sensor that is on, to ask a host that has just connected for again.
    std::vector<Request> active() const;

    // Turns every sensor off, as initialize does; returns what to tell the host.
    std::vector<Request> clear();

    bool has(int32_t handle) const;

    // Whether the framework has the sensor on.
    bool enabled(int32_t handle) const;

   private:
    struct State {
        Kind kind;
        bool enabled = false;
        int64_t periodNs = kDefaultPeriodNs;
        int32_t minDelayUs = 0;
    };
    Request request(const State& state) const;

    std::map<int32_t, State> mSensors;
};

// How an event carries its values.
enum class Payload { Vec3, Data, Scalar };

// One sensor event, in the shape of android.hardware.sensors.Event.
struct PlainEvent {
    int64_t timestampNs = 0;
    int32_t handle = 0;
    int32_t type = 0;
    Payload payload = Payload::Vec3;
    std::array<float, 5> values{};
};

// Turns readings into events on the guest's boot clock. Readings that arrive together keep
// their spacing on the host's clock, the newest at the time they arrived; each sensor's events
// only move forward in time.
class EventClock {
   public:
    std::vector<PlainEvent> stamp(const std::vector<Reading>& readings, int64_t nowNs);

   private:
    std::map<int32_t, int64_t> mLast;
};

}  // namespace dsa::sensors
