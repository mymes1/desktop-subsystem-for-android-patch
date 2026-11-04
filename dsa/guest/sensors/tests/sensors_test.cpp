// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The sensors HAL's rules: its messages against the checked-in protocol vectors, so they
// stay in step with the Rust and Kotlin sides; the Android sensors the host's become; what
// the host is asked for as the framework turns sensors on and off; the events readings
// become; and the writes into the framework's event queue. The protocol folder is the only
// argument.

#include <atomic>
#include <cstring>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

#include "event_queue.h"
#include "sensor_messages.h"
#include "sensor_table.h"
#include "test_main.h"

using namespace dsa::sensors;

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

HostSensor hostSensor(Kind kind, uint32_t minIntervalUs) {
    HostSensor sensor;
    sensor.kind = kind;
    sensor.name = "Host sensor";
    sensor.vendor = "DSA";
    sensor.maxRange = 10;
    sensor.resolution = 0.5f;
    sensor.minIntervalUs = minIntervalUs;
    return sensor;
}

Reading reading(Kind kind, uint64_t timestampUs, std::vector<float> values) {
    Reading r;
    r.kind = kind;
    r.timestampUs = timestampUs;
    r.values = std::move(values);
    return r;
}

// A queue's shared memory, as the framework lays it out: two counters, a ring and a flag word.
struct FakeQueue {
    std::atomic<uint64_t> read{0};
    std::atomic<uint64_t> write{0};
    std::vector<uint8_t> ring;
    std::atomic<uint32_t> flag{0};
    std::vector<uint32_t> wakes;

    explicit FakeQueue(size_t bytes) : ring(bytes, 0) {}

    EventQueue queue(size_t quantum) {
        return EventQueue(&read, &write, ring.data(), ring.size(), quantum, &flag,
                          [this](std::atomic<uint32_t>* word, uint32_t bits) {
                              word->fetch_or(bits);
                              wakes.push_back(bits);
                          });
    }
};

}  // namespace

TEST(fr26_activate_encodes_as_the_protocol_vector) {
    EXPECT_EQ(encodeActivate(Kind::Accelerometer, true, 20000), vector("sensor_activate"));
}

TEST(fr26_the_sensor_list_decodes_from_the_protocol_vector) {
    const std::vector<uint8_t> bytes = vector("sensor_list");
    const HostMessage message = decodeHostFrame(bytes.data(), bytes.size());
    const auto* list = std::get_if<SensorListMessage>(&message);
    EXPECT_TRUE(list != nullptr);
    if (list == nullptr) return;
    EXPECT_EQ(list->sensors.size(), 1u);
    if (list->sensors.empty()) return;
    EXPECT_TRUE(list->sensors[0].kind == Kind::Gyroscope);
    EXPECT_EQ(list->sensors[0].name, std::string("Gyro"));
    EXPECT_EQ(list->sensors[0].maxRange, 2.0f);
    EXPECT_EQ(list->sensors[0].minIntervalUs, 16000u);
}

TEST(fr26_readings_decode_from_the_protocol_vector) {
    const std::vector<uint8_t> bytes = vector("sensor_readings");
    const HostMessage message = decodeHostFrame(bytes.data(), bytes.size());
    const auto* readings = std::get_if<ReadingsMessage>(&message);
    EXPECT_TRUE(readings != nullptr);
    if (readings == nullptr) return;
    EXPECT_EQ(readings->readings.size(), 1u);
    if (readings->readings.empty()) return;
    EXPECT_TRUE(readings->readings[0].kind == Kind::Light);
    EXPECT_EQ(readings->readings[0].timestampUs, 1000u);
    EXPECT_EQ(readings->readings[0].values, (std::vector<float>{2.5f}));
}

TEST(fr26_a_frame_that_is_not_an_envelope_says_nothing) {
    const uint8_t garbage[] = {'n', 'o', 't', ' ', 'a', 'n', ' ', 'e', 'n', 'v'};
    const HostMessage message = decodeHostFrame(garbage, sizeof(garbage));
    EXPECT_TRUE(std::holds_alternative<std::monostate>(message));
}

TEST(fr26_each_host_sensor_becomes_one_android_sensor) {
    const std::vector<SensorDef> list = sensorList({
        hostSensor(Kind::Light, 100000),
        hostSensor(Kind::Accelerometer, 16000),
        hostSensor(Kind::Gyroscope, 16000),
        hostSensor(Kind::RotationVector, 16000),
        // A second accelerometer is not listed twice.
        hostSensor(Kind::Accelerometer, 5000),
    });
    EXPECT_EQ(list.size(), 4u);
    if (list.size() != 4) return;
    EXPECT_EQ(list[0].handle, 1);
    EXPECT_EQ(list[0].type, kTypeAccelerometer);
    EXPECT_EQ(list[0].typeAsString, std::string("android.sensor.accelerometer"));
    EXPECT_EQ(list[0].flags, kFlagContinuous);
    EXPECT_EQ(list[0].minDelayUs, 16000);
    EXPECT_EQ(list[0].maxDelayUs, kMaxDelayUs);
    EXPECT_EQ(list[0].maxRange, 10.0f);
    EXPECT_EQ(list[1].type, kTypeGyroscope);
    EXPECT_EQ(list[2].type, kTypeRotationVector);
    EXPECT_EQ(list[3].handle, 4);
    EXPECT_EQ(list[3].type, kTypeLight);
    EXPECT_EQ(list[3].flags, kFlagOnChange);
    EXPECT_EQ(list[3].minDelayUs, 100000);
}

TEST(fr26_a_sensor_the_host_lacks_is_not_listed) {
    EXPECT_TRUE(sensorList({}).empty());
    const std::vector<SensorDef> list = sensorList({hostSensor(Kind::Light, 100000)});
    EXPECT_EQ(list.size(), 1u);
    EXPECT_TRUE(!kindOfHandle(0));
    EXPECT_TRUE(!kindOfHandle(5));
}

TEST(fr26_a_sensor_runs_on_the_host_only_while_the_framework_has_it_on) {
    Activations activations(sensorList({hostSensor(Kind::Accelerometer, 16000)}));
    // The rate alone asks the host for nothing.
    EXPECT_TRUE(!activations.batch(1, 20'000'000));
    EXPECT_TRUE(!activations.enabled(1));
    EXPECT_TRUE(activations.activate(1, true) == (Request{Kind::Accelerometer, true, 20000}));
    EXPECT_TRUE(activations.enabled(1));
    // Faster than the sensor goes is as fast as it goes.
    EXPECT_TRUE(activations.batch(1, 1'000'000) == (Request{Kind::Accelerometer, true, 16000}));
    EXPECT_EQ(activations.active().size(), 1u);
    EXPECT_TRUE(activations.activate(1, false) == (Request{Kind::Accelerometer, false, 0}));
    EXPECT_TRUE(activations.active().empty());
    // A sensor this HAL does not have.
    EXPECT_TRUE(!activations.activate(2, true));
    EXPECT_TRUE(!activations.batch(2, 20'000'000));
}

TEST(fr26_initialize_turns_every_sensor_off) {
    Activations activations(
        sensorList({hostSensor(Kind::Accelerometer, 16000), hostSensor(Kind::Light, 100000)}));
    activations.activate(1, true);
    activations.activate(4, true);
    const std::vector<Request> off = activations.clear();
    EXPECT_EQ(off.size(), 2u);
    for (const Request& request : off) {
        EXPECT_TRUE(!request.enabled);
    }
    EXPECT_TRUE(activations.active().empty());
    // The default rate is back.
    EXPECT_TRUE(activations.activate(4, true) == (Request{Kind::Light, true, 200000}));
}

TEST(fr26_readings_become_events_on_the_boot_clock_in_order) {
    EventClock clock;
    const std::vector<PlainEvent> events = clock.stamp(
        {
            reading(Kind::Accelerometer, 1000, {0.0f, 0.0f, 9.80665f}),
            reading(Kind::Accelerometer, 3000, {0.0f, 1.0f, 9.0f}),
            reading(Kind::Light, 3000, {320.0f}),
            reading(Kind::RotationVector, 3000, {0.0f, 0.0f, 0.0f, 1.0f}),
            // Too few values for its kind.
            reading(Kind::Gyroscope, 3000, {1.0f}),
        },
        5'000'000'000);
    EXPECT_EQ(events.size(), 4u);
    if (events.size() != 4) return;
    // The newest reading is now; the one 2 ms older on the host is 2 ms older here.
    EXPECT_EQ(events[0].timestampNs, 4'998'000'000);
    EXPECT_EQ(events[1].timestampNs, 5'000'000'000);
    EXPECT_EQ(events[0].type, kTypeAccelerometer);
    EXPECT_TRUE(events[0].payload == Payload::Vec3);
    EXPECT_EQ(events[0].values[2], 9.80665f);
    EXPECT_EQ(events[2].type, kTypeLight);
    EXPECT_TRUE(events[2].payload == Payload::Scalar);
    EXPECT_EQ(events[2].values[0], 320.0f);
    EXPECT_EQ(events[3].type, kTypeRotationVector);
    EXPECT_TRUE(events[3].payload == Payload::Data);
    EXPECT_EQ(events[3].values[3], 1.0f);
    // The heading's accuracy is not known.
    EXPECT_EQ(events[3].values[4], -1.0f);

    // A late batch whose clock says earlier still comes after what the framework has.
    const std::vector<PlainEvent> late = clock.stamp({reading(Kind::Light, 0, {1.0f})}, 4'000'000'000);
    EXPECT_EQ(late.size(), 1u);
    if (late.empty()) return;
    EXPECT_TRUE(late[0].timestampNs > events[2].timestampNs);
}

TEST(fr26_events_wrap_around_the_framework_queue) {
    FakeQueue memory(4 * 8);
    EventQueue queue = memory.queue(8);
    EXPECT_EQ(queue.capacityItems(), 4u);
    const uint64_t first[3] = {1, 2, 3};
    EXPECT_EQ(queue.write(first, 3), 3u);
    EXPECT_EQ(memory.write.load(), 24u);
    // The framework reads two.
    memory.read.store(16);
    const uint64_t second[3] = {4, 5, 6};
    EXPECT_EQ(queue.write(second, 3), 3u);
    EXPECT_EQ(memory.write.load(), 48u);
    uint64_t slots[4];
    std::memcpy(slots, memory.ring.data(), sizeof(slots));
    // 4 went in the last slot, 5 and 6 wrapped round to the first two.
    EXPECT_EQ(slots[0], 5u);
    EXPECT_EQ(slots[1], 6u);
    EXPECT_EQ(slots[2], 3u);
    EXPECT_EQ(slots[3], 4u);
    queue.wake(1);
    EXPECT_EQ(memory.flag.load(), 1u);
    EXPECT_EQ(memory.wakes, (std::vector<uint32_t>{1}));
}

TEST(fr26_a_full_queue_drops_events_rather_than_overwrite_them) {
    FakeQueue memory(2 * 8);
    EventQueue queue = memory.queue(8);
    const uint64_t items[3] = {7, 8, 9};
    EXPECT_EQ(queue.write(items, 3), 2u);
    EXPECT_EQ(queue.write(items, 1), 0u);
    uint64_t slots[2];
    std::memcpy(slots, memory.ring.data(), sizeof(slots));
    EXPECT_EQ(slots[0], 7u);
    EXPECT_EQ(slots[1], 8u);
    EXPECT_EQ(memory.write.load(), 16u);
}

int main(int argc, char** argv) {
    if (argc != 2) {
        std::fprintf(stderr, "usage: %s <dsa/protocol>\n", argv[0]);
        return 2;
    }
    gProtocolDir = argv[1];
    return ::dsa::test::runAll();
}
