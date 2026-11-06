// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "location_messages.h"

#include <cmath>

#include "control.pb.h"

namespace dsa::gnss {

namespace v1 = ::dsa::control::v1;

namespace {

std::vector<uint8_t> bytesOf(const v1::Envelope& envelope) {
    const std::string bytes = envelope.SerializeAsString();
    return std::vector<uint8_t>(bytes.begin(), bytes.end());
}

bool positive(float value) { return std::isfinite(value) && value > 0; }

std::optional<Fix> fixOf(const v1::LocationFix& from) {
    const double lat = from.latitude_degrees();
    const double lon = from.longitude_degrees();
    if (!std::isfinite(lat) || lat < -90 || lat > 90 || !std::isfinite(lon) || lon < -180 || lon > 180) {
        return std::nullopt;
    }
    Fix fix;
    fix.flags = flags::kLatLong;
    fix.latitudeDegrees = lat;
    fix.longitudeDegrees = lon;
    if (from.has_altitude() && std::isfinite(from.altitude_meters())) {
        fix.flags |= flags::kAltitude;
        fix.altitudeMeters = from.altitude_meters();
    }
    if (from.has_speed() && std::isfinite(from.speed_meters_per_second()) && from.speed_meters_per_second() >= 0) {
        fix.flags |= flags::kSpeed;
        fix.speedMetersPerSec = from.speed_meters_per_second();
    }
    if (from.has_bearing() && std::isfinite(from.bearing_degrees())) {
        fix.flags |= flags::kBearing;
        double bearing = std::fmod(static_cast<double>(from.bearing_degrees()), 360.0);
        fix.bearingDegrees = bearing < 0 ? bearing + 360.0 : bearing;
    }
    if (positive(from.horizontal_accuracy_meters())) {
        fix.flags |= flags::kHorizontalAccuracy;
        fix.horizontalAccuracyMeters = from.horizontal_accuracy_meters();
    }
    if (positive(from.vertical_accuracy_meters())) {
        fix.flags |= flags::kVerticalAccuracy;
        fix.verticalAccuracyMeters = from.vertical_accuracy_meters();
    }
    fix.timestampMillis = static_cast<int64_t>(from.unix_time_ms());
    return fix;
}

Access accessOf(v1::LocationAccess access) {
    switch (access) {
        case v1::LOCATION_ACCESS_ALLOWED:
            return Access::Allowed;
        case v1::LOCATION_ACCESS_DENIED:
            return Access::Denied;
        case v1::LOCATION_ACCESS_UNAVAILABLE:
            return Access::Unavailable;
        default:
            return Access::Unknown;
    }
}

}  // namespace

const char* describe(Access access) {
    switch (access) {
        case Access::Allowed:
            return "allowed";
        case Access::Denied:
            return "turned off in the host's privacy settings";
        case Access::Unavailable:
            return "not available on the host";
        case Access::Unknown:
            break;
    }
    return "unknown";
}

std::optional<HostLocation> decodeHostLocation(const uint8_t* data, size_t size) {
    v1::Envelope envelope;
    if (!envelope.ParseFromArray(data, static_cast<int>(size))) {
        return std::nullopt;
    }
    HostLocation location;
    switch (envelope.body_case()) {
        case v1::Envelope::kLocationFix:
            location.fix = fixOf(envelope.location_fix());
            if (!location.fix) {
                return std::nullopt;
            }
            return location;
        case v1::Envelope::kLocationStatus:
            location.access = accessOf(envelope.location_status().access());
            location.detail = envelope.location_status().detail();
            return location;
        default:
            return std::nullopt;
    }
}

std::vector<uint8_t> encodeLocationRequest(bool active, uint32_t intervalMs) {
    v1::Envelope envelope;
    v1::LocationRequest* request = envelope.mutable_location_request();
    request->set_active(active);
    request->set_interval_ms(intervalMs);
    return bytesOf(envelope);
}

}  // namespace dsa::gnss
