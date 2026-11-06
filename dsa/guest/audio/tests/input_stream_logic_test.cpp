// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The input stream's commands: the host's sound reaches the framework at the stream's own
// pace, a host that sends nothing leaves the stream running and silent, a muted microphone
// is silent, and the state machine is the one the HAL interface gives an input.

#include <cstdint>
#include <cstring>
#include <vector>

#include "input_stream_logic.h"
#include "test_main.h"

using namespace dsa::audio;

namespace {

constexpr int64_t kMs = 1000000;
constexpr uint32_t kBurst = 960;  // 20 ms at 48 kHz
constexpr int32_t kBurstBytes = kBurst * 4;

class FakeSource final : public CaptureSource {
   public:
    size_t pull(uint8_t* out, size_t frames) override {
        const size_t have = frames < available ? frames : available;
        std::memset(out, value, have * 4);
        std::memset(out + have * 4, 0, (frames - have) * 4);
        available -= have;
        pulled += frames;
        return have;
    }
    void reset() override { ++resets; }
    uint32_t hostDelayFrames() const override { return 0; }

    size_t available = 1 << 30;
    uint8_t value = 7;
    size_t pulled = 0;
    int resets = 0;
};

class FakeClock final : public Clock {
   public:
    int64_t nowNs() override { return now; }
    void sleepUntilNs(int64_t deadline) override {
        sleeps.push_back(deadline - now);
        if (deadline > now) now = deadline;
    }

    int64_t now = 1000 * kMs;
    std::vector<int64_t> sleeps;
};

class FakeWriter final : public PcmWriter {
   public:
    size_t availableToWrite() const override { return room; }
    bool write(const uint8_t* data, size_t size) override {
        if (size > room) return false;
        written.insert(written.end(), data, data + size);
        return true;
    }

    size_t room = 1 << 20;
    std::vector<uint8_t> written;
};

Command cmd(Command::Kind kind, int32_t bytes = 0) {
    Command c;
    c.kind = kind;
    c.bytes = bytes;
    return c;
}

struct Fixture {
    FakeSource source;
    FakeClock clock;
    FakeWriter writer;
    bool muted = false;
    InputStreamLogic logic{source, clock, StreamFormat{48000, 4, kBurst}, [this] { return muted; }};

    Reply run(Command::Kind kind, int32_t bytes = 0) { return logic.handle(cmd(kind, bytes), writer); }
};

}  // namespace

TEST(fr26_the_hosts_sound_reaches_the_framework) {
    Fixture f;
    EXPECT_EQ(f.run(Command::Kind::Start).status, kStatusOk);
    EXPECT_EQ(static_cast<int>(f.logic.state()), static_cast<int>(StreamState::Idle));
    const Reply r = f.run(Command::Kind::Burst, kBurstBytes);
    EXPECT_EQ(r.status, kStatusOk);
    EXPECT_EQ(r.fmqByteCount, kBurstBytes);
    EXPECT_EQ(static_cast<int>(r.state), static_cast<int>(StreamState::Active));
    EXPECT_EQ(f.writer.written.size(), static_cast<size_t>(kBurstBytes));
    EXPECT_EQ(f.writer.written[0], 7);
    EXPECT_EQ(r.observableFrames, static_cast<int64_t>(kBurst));
}

TEST(fr26_a_burst_returns_once_its_frames_were_captured) {
    Fixture f;
    f.run(Command::Kind::Start);
    const int64_t start = f.clock.now;
    for (int i = 0; i < 5; ++i) {
        f.run(Command::Kind::Burst, kBurstBytes);
    }
    // Five bursts of 20 ms take 100 ms of the guest's clock, whatever the host does.
    EXPECT_EQ(f.clock.now - start, 100 * kMs);
}

TEST(fr26_a_host_that_sends_nothing_leaves_the_stream_running_and_silent) {
    Fixture f;
    f.source.available = 0;
    f.run(Command::Kind::Start);
    const int64_t start = f.clock.now;
    const Reply r = f.run(Command::Kind::Burst, kBurstBytes);
    EXPECT_EQ(r.status, kStatusOk);
    EXPECT_EQ(r.fmqByteCount, kBurstBytes);
    EXPECT_EQ(f.clock.now - start, 20 * kMs);
    bool silent = true;
    for (uint8_t b : f.writer.written) silent = silent && b == 0;
    EXPECT_TRUE(silent);
}

TEST(fr26_a_muted_microphone_is_silent_in_time) {
    Fixture f;
    f.muted = true;
    f.run(Command::Kind::Start);
    const Reply r = f.run(Command::Kind::Burst, kBurstBytes);
    EXPECT_EQ(r.fmqByteCount, kBurstBytes);
    bool silent = true;
    for (uint8_t b : f.writer.written) silent = silent && b == 0;
    EXPECT_TRUE(silent);
}

TEST(fr26_a_late_reader_counts_an_overrun_and_is_not_rushed) {
    Fixture f;
    f.run(Command::Kind::Start);
    f.run(Command::Kind::Burst, kBurstBytes);
    // The reader comes back 200 ms later: what was captured meanwhile is gone.
    f.clock.now += 200 * kMs;
    f.clock.sleeps.clear();
    const Reply r = f.run(Command::Kind::Burst, kBurstBytes);
    EXPECT_TRUE(r.xrunFrames > 0);
    // The next burst waits its own 20 ms again.
    EXPECT_EQ(f.clock.sleeps.size(), 1u);
    EXPECT_EQ(f.clock.sleeps[0], 20 * kMs);
}

TEST(fr26_the_input_state_machine_follows_the_hal_interface) {
    Fixture f;
    // A burst before the start is the wrong state.
    EXPECT_EQ(f.run(Command::Kind::Burst, kBurstBytes).status, kStatusInvalidOperation);
    EXPECT_EQ(f.run(Command::Kind::Start).status, kStatusOk);
    // Starting drops what was queued while the stream stood by.
    EXPECT_EQ(f.source.resets, 1);
    EXPECT_EQ(f.run(Command::Kind::Standby).status, kStatusOk);
    EXPECT_EQ(static_cast<int>(f.logic.state()), static_cast<int>(StreamState::Standby));
    f.run(Command::Kind::Start);
    f.run(Command::Kind::Burst, kBurstBytes);
    EXPECT_EQ(f.run(Command::Kind::Standby).status, kStatusInvalidOperation);
    EXPECT_EQ(f.run(Command::Kind::Pause).status, kStatusOk);
    EXPECT_EQ(static_cast<int>(f.logic.state()), static_cast<int>(StreamState::Paused));
    EXPECT_EQ(f.run(Command::Kind::Flush).status, kStatusOk);
    EXPECT_EQ(static_cast<int>(f.logic.state()), static_cast<int>(StreamState::Standby));
    f.run(Command::Kind::Start);
    f.run(Command::Kind::Burst, kBurstBytes);
    EXPECT_EQ(f.run(Command::Kind::Drain).status, kStatusOk);
    EXPECT_EQ(static_cast<int>(f.logic.state()), static_cast<int>(StreamState::Draining));
    f.run(Command::Kind::Burst, kBurstBytes);
    EXPECT_EQ(static_cast<int>(f.logic.state()), static_cast<int>(StreamState::Standby));
    EXPECT_EQ(f.run(Command::Kind::Unsupported).status, kStatusInvalidOperation);
}

TEST(fr26_a_burst_writes_no_more_than_the_queue_has_room_for) {
    Fixture f;
    f.writer.room = 400;
    f.run(Command::Kind::Start);
    const Reply r = f.run(Command::Kind::Burst, kBurstBytes);
    EXPECT_EQ(r.fmqByteCount, 400);
    EXPECT_EQ(f.logic.framesRead(), 100u);
}

int main() { return dsa::test::runAll(); }
