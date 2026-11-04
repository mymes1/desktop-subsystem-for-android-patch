// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The sensor port's messages, through the codec generated from dsa/protocol/control.proto.
// Each frame is one encoded Envelope, sent with a 4-byte little-endian length before it.

#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <variant>
#include <vector>

namespace dsa::sensors {

// A sensor the host shows to the guest; the numbers are the protocol's SensorKind.
enum class Kind : int32_t {
    Accelerometer = 1,
    Gyroscope = 2,
    RotationVector = 3,
    Light = 4,
};

// One sensor the host has, as its SensorList describes it.
struct HostSensor {
    Kind kind = Kind::Accelerometer;
    std::string name;
    std::string vendor;
    float maxRange = 0;
    float resolution = 0;
    uint32_t minIntervalUs = 0;
};

// One reading, already in Android's units and on its axes. `timestampUs` is on the host's
// monotonic clock.
struct Reading {
    Kind kind = Kind::Accelerometer;
    uint64_t timestampUs = 0;
    std::vector<float> values;
};

struct Accepted {};
struct Rejected {
    std::string detail;
};
struct SensorListMessage {
    std::vector<HostSensor> sensors;
};
struct ReadingsMessage {
    std::vector<Reading> readings;
};

// What a frame from the host says; monostate for a frame that cannot be decoded or that the
// sensor port does not use.
using HostMessage = std::variant<std::monostate, Accepted, Rejected, SensorListMessage, ReadingsMessage>;

HostMessage decodeHostFrame(const uint8_t* data, size_t size);

// The Hello that opens the sensor port: ROLE_GUEST_SENSORS, this build's protocol version and
// schema hash.
std::vector<uint8_t> encodeHello(const std::string& build);

// Turns the host's sensor of `kind` on, with a reading about every `intervalUs`, or off.
std::vector<uint8_t> encodeActivate(Kind kind, bool enabled, uint32_t intervalUs);

// `frame` with its 4-byte little-endian length before it, as the transport carries it.
std::vector<uint8_t> withLength(const std::vector<uint8_t>& frame);

}  // namespace dsa::sensors
