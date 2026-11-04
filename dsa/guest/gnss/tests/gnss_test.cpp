// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The GNSS HAL's rules: its channel messages against the checked-in protocol vectors, so they
// stay in step with the Rust and Kotlin sides; the GnssLocation values a host position
// becomes; and when the host is asked for locations and which positions reach the framework.
// The protocol folder is the only argument.

#include <fstream>
#include <sstream>
#include <string>
#include <vector>

#include "control.pb.h"
#include "location_messages.h"
#include "location_session.h"
#include "test_main.h"

using namespace dsa::gnss;
namespace v1 = ::dsa::control::v1;

namespace {

std::string gProtocolDir;

std::vector<uint8_t> vector(const std::string& name) {
    std::ifstream in(gProtocolDir + "/vectors/control.vectors");
    std::string line;
    while (std::getline(in, line)) {
        std::istringstream fields(line);
        std::string key, hex;
        fields >> key >> hex;
        if (key != name) {
            continue;
        }
        std::vector<uint8_t> bytes;
        for (size_t i = 0; hex != "-" && i + 1 < hex.size(); i += 2) {
            bytes.push_back(static_cast<uint8_t>(std::stoi(hex.substr(i, 2), nullptr, 16)));
        }
        return bytes;
    }
    ::dsa::test::fail(__FILE__, __LINE__, "the vectors have no case " + name);
    return {};
}

std::vector<uint8_t> bytesOf(const v1::Envelope& envelope) {
    const std::string bytes = envelope.SerializeAsString();
    return std::vector<uint8_t>(bytes.begin(), bytes.end());
}

std::optional<HostLocation> decode(const std::vector<uint8_t>& bytes) {
    return decodeHostLocation(bytes.data(), bytes.size());
}

HostLocation status(Access access) {
    HostLocation location;
    location.access = access;
    return location;
}

// The host's position in the protocol vector "location_fix", as the framework gets it.
Fix seoul() {
    Fix fix;
    fix.flags = flags::kLatLong | flags::kAltitude | flags::kSpeed | flags::kBearing |
                flags::kHorizontalAccuracy | flags::kVerticalAccuracy;
    fix.latitudeDegrees = 37.5;
    fix.longitudeDegrees = 127.0;
    fix.altitudeMeters = 50.0;
    fix.speedMetersPerSec = 2.5;
    fix.bearingDegrees = 90.0;
    fix.horizontalAccuracyMeters = 10.0;
    fix.verticalAccuracyMeters = 3.0;
    fix.timestampMillis = 1700000000000LL;
    return fix;
}

HostLocation at(const Fix& fix) {
    HostLocation location;
    location.fix = fix;
    return location;
}

}  // namespace

TEST(fr26_a_location_request_encodes_as_the_protocol_vector) {
    EXPECT_EQ(encodeLocationRequest(true, 1000), vector("location_request_active"));
}

TEST(fr26_a_host_fix_becomes_the_same_gnss_location_values) {
    const auto location = decode(vector("location_fix"));
    EXPECT_TRUE(location.has_value());
    if (location) {
        EXPECT_TRUE(location->fix.has_value());
        EXPECT_TRUE(!location->access.has_value());
        if (location->fix) {
            EXPECT_EQ(*location->fix, seoul());
        }
    }
}

TEST(fr26_values_the_host_does_not_know_are_flagged_as_unknown) {
    v1::Envelope envelope;
    v1::LocationFix* fix = envelope.mutable_location_fix();
    fix->set_latitude_degrees(-33.9);
    fix->set_longitude_degrees(151.2);
    fix->set_unix_time_ms(5);
    const auto location = decode(bytesOf(envelope));
    EXPECT_TRUE(location.has_value() && location->fix.has_value());
    if (location && location->fix) {
        EXPECT_EQ(location->fix->flags, flags::kLatLong);
        EXPECT_EQ(location->fix->latitudeDegrees, -33.9);
        EXPECT_EQ(location->fix->timestampMillis, 5);
    }
}

TEST(fr26_a_position_that_is_not_on_earth_is_dropped) {
    v1::Envelope envelope;
    envelope.mutable_location_fix()->set_latitude_degrees(91);
    EXPECT_TRUE(!decode(bytesOf(envelope)).has_value());
    const uint8_t garbage[] = {0xff, 0xff, 0xff};
    EXPECT_TRUE(!decodeHostLocation(garbage, sizeof garbage).has_value());
    EXPECT_TRUE(!decode(vector("task_removed")).has_value());
}

TEST(fr26_a_denied_status_decodes_with_the_hosts_words) {
    const auto location = decode(vector("location_status_denied"));
    EXPECT_TRUE(location.has_value());
    if (location) {
        EXPECT_TRUE(!location->fix.has_value());
        EXPECT_TRUE(location->access == Access::Denied);
    }
}

TEST(fr26_the_host_is_asked_for_locations_only_between_start_and_stop) {
    LocationSession session;
    EXPECT_TRUE(!session.stop().has_value());
    EXPECT_TRUE(!session.reconnected().has_value());
    EXPECT_TRUE(!session.setInterval(2000).has_value());

    EXPECT_EQ(session.start(), encodeLocationRequest(true, 2000));
    EXPECT_EQ(session.reconnected(), std::optional(encodeLocationRequest(true, 2000)));
    EXPECT_EQ(session.setInterval(1000), std::optional(encodeLocationRequest(true, 1000)));
    EXPECT_TRUE(!session.setInterval(1000).has_value());
    // The framework's fastest is kept to what a host gives.
    EXPECT_EQ(session.setInterval(1), std::optional(encodeLocationRequest(true, LocationSession::kMinIntervalMs)));

    EXPECT_EQ(session.stop(), std::optional(encodeLocationRequest(false, 0)));
    EXPECT_TRUE(!session.stop().has_value());
}

TEST(fr26_positions_reach_the_framework_only_while_the_engine_runs) {
    LocationSession session;
    EXPECT_TRUE(!session.fromHost(at(seoul())).has_value());
    session.start();
    EXPECT_EQ(session.fromHost(at(seoul())), std::optional(seoul()));
    session.stop();
    EXPECT_TRUE(!session.fromHost(at(seoul())).has_value());
}

TEST(fr26_no_position_reaches_the_framework_while_the_host_denies_location) {
    LocationSession session;
    session.start();
    EXPECT_TRUE(!session.fromHost(status(Access::Denied)).has_value());
    EXPECT_TRUE(session.access() == Access::Denied);
    EXPECT_TRUE(!session.fromHost(at(seoul())).has_value());
    EXPECT_TRUE(!session.fromHost(status(Access::Unavailable)).has_value());
    EXPECT_TRUE(!session.fromHost(at(seoul())).has_value());

    // The user turned location on again.
    session.fromHost(status(Access::Allowed));
    EXPECT_EQ(session.fromHost(at(seoul())), std::optional(seoul()));
}

int main(int argc, char** argv) {
    if (argc != 2) {
        std::fprintf(stderr, "usage: %s <dsa/protocol folder>\n", argv[0]);
        return 2;
    }
    gProtocolDir = argv[1];
    return ::dsa::test::runAll();
}
