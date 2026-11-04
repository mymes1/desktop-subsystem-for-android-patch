// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "sensor_messages.h"

#include <cstdlib>

#include "control.pb.h"
#include "control_schema.h"

namespace dsa::sensors {

namespace v1 = ::dsa::control::v1;

namespace {

std::vector<uint8_t> encode(const v1::Envelope& envelope) {
    const std::string bytes = envelope.SerializeAsString();
    return std::vector<uint8_t>(bytes.begin(), bytes.end());
}

std::string schemaHash() {
    std::string hash;
    const char* hex = v1::kSchemaHashHex;
    for (size_t i = 0; hex[i] != '\0' && hex[i + 1] != '\0'; i += 2) {
        const char pair[3] = {hex[i], hex[i + 1], '\0'};
        hash.push_back(static_cast<char>(std::strtoul(pair, nullptr, 16)));
    }
    return hash;
}

bool knownKind(int32_t kind) {
    return kind >= static_cast<int32_t>(Kind::Accelerometer) && kind <= static_cast<int32_t>(Kind::Light);
}

}  // namespace

HostMessage decodeHostFrame(const uint8_t* data, size_t size) {
    v1::Envelope envelope;
    if (!envelope.ParseFromArray(data, static_cast<int>(size))) {
        return std::monostate{};
    }
    switch (envelope.body_case()) {
        case v1::Envelope::kHelloAccepted:
            return Accepted{};
        case v1::Envelope::kHelloRejected:
            return Rejected{envelope.hello_rejected().detail()};
        case v1::Envelope::kSensorList: {
            SensorListMessage list;
            for (const v1::SensorDescription& s : envelope.sensor_list().sensors()) {
                const int32_t kind = static_cast<int32_t>(s.kind());
                if (!knownKind(kind)) {
                    continue;
                }
                HostSensor sensor;
                sensor.kind = static_cast<Kind>(kind);
                sensor.name = s.name();
                sensor.vendor = s.vendor();
                sensor.maxRange = s.max_range();
                sensor.resolution = s.resolution();
                sensor.minIntervalUs = s.min_interval_us();
                list.sensors.push_back(std::move(sensor));
            }
            return list;
        }
        case v1::Envelope::kSensorReadings: {
            ReadingsMessage readings;
            for (const v1::SensorReading& r : envelope.sensor_readings().readings()) {
                const int32_t kind = static_cast<int32_t>(r.kind());
                if (!knownKind(kind)) {
                    continue;
                }
                Reading reading;
                reading.kind = static_cast<Kind>(kind);
                reading.timestampUs = r.timestamp_us();
                reading.values.assign(r.values().begin(), r.values().end());
                readings.readings.push_back(std::move(reading));
            }
            return readings;
        }
        default:
            return std::monostate{};
    }
}

std::vector<uint8_t> encodeHello(const std::string& build) {
    v1::Envelope envelope;
    v1::Hello* hello = envelope.mutable_hello();
    hello->set_protocol_version(v1::PROTOCOL_VERSION_CURRENT);
    hello->set_schema_hash(schemaHash());
    hello->set_role(v1::ROLE_GUEST_SENSORS);
    hello->set_build(build);
    return encode(envelope);
}

std::vector<uint8_t> encodeActivate(Kind kind, bool enabled, uint32_t intervalUs) {
    v1::Envelope envelope;
    v1::SensorActivate* activate = envelope.mutable_sensor_activate();
    activate->set_kind(static_cast<v1::SensorKind>(kind));
    activate->set_enabled(enabled);
    activate->set_interval_us(intervalUs);
    return encode(envelope);
}

std::vector<uint8_t> withLength(const std::vector<uint8_t>& frame) {
    std::vector<uint8_t> out;
    out.reserve(frame.size() + 4);
    const uint32_t size = static_cast<uint32_t>(frame.size());
    for (int shift = 0; shift < 32; shift += 8) {
        out.push_back(static_cast<uint8_t>(size >> shift));
    }
    out.insert(out.end(), frame.begin(), frame.end());
    return out;
}

}  // namespace dsa::sensors
