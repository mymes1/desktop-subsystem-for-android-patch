// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The control channel messages the GNSS HAL reads and writes, through the codec generated
// from dsa/protocol/control.proto. Each frame is one encoded Envelope. Nothing here needs
// binder, so the rules build and run on any machine.

#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <vector>

namespace dsa::gnss {

// android.hardware.gnss's GnssLocation flags: which of its values are known.
namespace flags {
constexpr int32_t kLatLong = 0x0001;
constexpr int32_t kAltitude = 0x0002;
constexpr int32_t kSpeed = 0x0004;
constexpr int32_t kBearing = 0x0008;
constexpr int32_t kHorizontalAccuracy = 0x0010;
constexpr int32_t kVerticalAccuracy = 0x0020;
}  // namespace flags

// One position from the host, in GnssLocation's terms, so the service only copies it.
struct Fix {
    int32_t flags = 0;
    double latitudeDegrees = 0;
    double longitudeDegrees = 0;
    double altitudeMeters = 0;
    double speedMetersPerSec = 0;
    double bearingDegrees = 0;
    double horizontalAccuracyMeters = 0;
    double verticalAccuracyMeters = 0;
    // When the host took it, in milliseconds since the Unix epoch (UTC).
    int64_t timestampMillis = 0;

    bool operator==(const Fix& other) const {
        return flags == other.flags && latitudeDegrees == other.latitudeDegrees &&
               longitudeDegrees == other.longitudeDegrees && altitudeMeters == other.altitudeMeters &&
               speedMetersPerSec == other.speedMetersPerSec && bearingDegrees == other.bearingDegrees &&
               horizontalAccuracyMeters == other.horizontalAccuracyMeters &&
               verticalAccuracyMeters == other.verticalAccuracyMeters &&
               timestampMillis == other.timestampMillis;
    }
};

// Whether the host lets the guest have its location (LocationStatus).
enum class Access {
    Unknown,
    Allowed,
    // Turned off for DSA, or for the whole computer, in the host's privacy settings.
    Denied,
    // No location device on the host, or its location service failed.
    Unavailable,
};

const char* describe(Access access);

// What a frame from the host says about the location: a position, or whether the host can
// give one, with the host's words for why.
struct HostLocation {
    std::optional<Fix> fix;
    std::optional<Access> access;
    std::string detail;
};

// What the frame carries about the location; nothing for any other frame and for one that
// cannot be decoded. A position that is not on Earth carries nothing either.
std::optional<HostLocation> decodeHostLocation(const uint8_t* data, size_t size);

// An Envelope carrying a LocationRequest: whether a location is wanted, and how often.
std::vector<uint8_t> encodeLocationRequest(bool active, uint32_t intervalMs);

}  // namespace dsa::gnss
