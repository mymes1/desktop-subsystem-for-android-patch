// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The output stream's commands: the sound reaches the host in order and at the stream's own
// pace, the host follows the stream's state, and the position the framework reads never
// goes back or runs ahead of what was written.

#include <cstdint>
#include <vector>

#include "output_stream_logic.h"
#include "test_main.h"

using namespace dsa::audio;

namespace {

constexpr int64_t kMs = 1000000;
constexpr uint32_t kBurst = 960;  // 20 ms at 48 kHz
constexpr int32_t kBurstBytes = kBurst * 4;

struct Sent {
    uint64_t position;
    size_t bytes;
};

class FakeSink final : public PcmSink {
   public:
    void pcm(uint64_t position, const uint8_t*, size_t size) override { sent.push_back({position, size}); }
    void command(HostCommand command) override { commands.push_back(command); }
    uint32_t hostDelayFrames() const override { return delay; }

    std::vector<Sent> sent;
    std::vector<HostCommand> commands;
    uint32_t delay = 0;
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

// Whatever the framework wrote: as many bytes as asked for.
class FakeSource final : public PcmSource {
   public:
    size_t read(uint8_t* out, size_t max) override {
        const size_t n = max < available ? max : available;
        for (size_t i = 0; i < n; ++i) out[i] = 0;
        available -= n;
        return n;
    }

    size_t available = 1 << 30;
};

Command command(Command::Kind kind, int32_t bytes = 0) {
    Command c;
    c.kind = kind;
    c.bytes = bytes;
    return c;
}

StreamFormat format() {
    StreamFormat f;
    f.bufferFrames = kBurst;
    return f;
}

}  // namespace

TEST(pr4_the_first_burst_starts_the_host_and_sends_the_sound_from_frame_zero) {
    FakeSink sink;
    FakeClock clock;
    FakeSource source;
    OutputStreamLogic logic(sink, clock, format());
    EXPECT_EQ(logic.handle(command(Command::Kind::Start), source).state, StreamState::Idle);
    const Reply reply = logic.handle(command(Command::Kind::Burst, kBurstBytes), source);
    EXPECT_EQ(reply.status, kStatusOk);
    EXPECT_EQ(reply.state, StreamState::Active);
    EXPECT_EQ(reply.fmqByteCount, kBurstBytes);
    EXPECT_EQ(sink.commands.size(), 1u);
    EXPECT_TRUE(sink.commands[0] == HostCommand::Start);
    EXPECT_EQ(sink.sent.size(), 1u);
    EXPECT_EQ(sink.sent[0].position, 0u);
    EXPECT_EQ(sink.sent[0].bytes, static_cast<size_t>(kBurstBytes));
}

TEST(pr4_bursts_are_paced_by_the_guest_clock_not_the_host) {
    // The mixer writes as fast as the stream returns. After the first buffer, each burst
    // waits until the play head is one buffer behind, so 20 ms of sound takes 20 ms.
    FakeSink sink;
    FakeClock clock;
    FakeSource source;
    OutputStreamLogic logic(sink, clock, format());
    logic.handle(command(Command::Kind::Start), source);
    const int64_t start = clock.now;
    for (int i = 0; i < 51; ++i) {
        logic.handle(command(Command::Kind::Burst, kBurstBytes), source);
    }
    // 51 bursts: the first is free, the next 50 take 20 ms each.
    EXPECT_EQ(clock.now - start, 50 * 20 * kMs);
    EXPECT_EQ(logic.framesWritten(), 51u * kBurst);
    for (size_t i = 0; i < sink.sent.size(); ++i) {
        EXPECT_EQ(sink.sent[i].position, i * kBurst);
    }
}

TEST(pr4_a_late_mixer_is_counted_as_an_underrun_and_not_rushed) {
    FakeSink sink;
    FakeClock clock;
    FakeSource source;
    OutputStreamLogic logic(sink, clock, format());
    logic.handle(command(Command::Kind::Start), source);
    logic.handle(command(Command::Kind::Burst, kBurstBytes), source);
    logic.handle(command(Command::Kind::Burst, kBurstBytes), source);
    // The mixer stalls for 100 ms: 4800 frames' worth of time, with 960 written ahead.
    clock.now += 100 * kMs;
    clock.sleeps.clear();
    const Reply reply = logic.handle(command(Command::Kind::Burst, kBurstBytes), source);
    EXPECT_TRUE(reply.xrunFrames > 0);
    // The next burst does not return at once to catch up: the stream restarted its clock.
    logic.handle(command(Command::Kind::Burst, kBurstBytes), source);
    EXPECT_EQ(clock.sleeps.size(), 1u);
    EXPECT_TRUE(clock.sleeps.empty() || clock.sleeps[0] > 0);
}

TEST(pr4_the_position_never_goes_back_nor_passes_what_was_written) {
    FakeSink sink;
    FakeClock clock;
    FakeSource source;
    OutputStreamLogic logic(sink, clock, format());
    logic.handle(command(Command::Kind::Start), source);
    int64_t last = 0;
    for (int i = 0; i < 20; ++i) {
        const Reply r = logic.handle(command(Command::Kind::Burst, kBurstBytes), source);
        EXPECT_TRUE(r.observableFrames >= last);
        EXPECT_TRUE(r.observableFrames <= static_cast<int64_t>(logic.framesWritten()));
        last = r.observableFrames;
    }
    clock.now += 1000 * kMs;
    const Reply after = logic.handle(command(Command::Kind::GetStatus), source);
    EXPECT_TRUE(after.observableFrames >= last);
    EXPECT_TRUE(after.observableFrames <= static_cast<int64_t>(logic.framesWritten()));
}

TEST(pr4_the_reported_latency_includes_the_host_queue) {
    FakeSink sink;
    sink.delay = 48000 * 60 / 1000;  // the host says 60 ms
    FakeClock clock;
    OutputStreamLogic logic(sink, clock, format());
    // 20 ms of the stream's own buffer and 60 ms on the host.
    EXPECT_EQ(logic.latencyMs(), 80);
}

TEST(pr4_pause_flush_and_standby_reach_the_host) {
    FakeSink sink;
    FakeClock clock;
    FakeSource source;
    OutputStreamLogic logic(sink, clock, format());
    logic.handle(command(Command::Kind::Start), source);
    logic.handle(command(Command::Kind::Burst, kBurstBytes), source);
    EXPECT_EQ(logic.handle(command(Command::Kind::Pause), source).state, StreamState::Paused);
    EXPECT_EQ(logic.handle(command(Command::Kind::Flush), source).state, StreamState::Idle);
    EXPECT_EQ(logic.handle(command(Command::Kind::Standby), source).state, StreamState::Standby);
    EXPECT_EQ(sink.commands.size(), 4u);
    EXPECT_TRUE(sink.commands[1] == HostCommand::Pause);
    EXPECT_TRUE(sink.commands[2] == HostCommand::Flush);
    EXPECT_TRUE(sink.commands[3] == HostCommand::Standby);
}

TEST(pr4_commands_out_of_turn_are_refused_without_changing_state) {
    FakeSink sink;
    FakeClock clock;
    FakeSource source;
    OutputStreamLogic logic(sink, clock, format());
    EXPECT_EQ(logic.handle(command(Command::Kind::Pause), source).status, kStatusInvalidOperation);
    EXPECT_EQ(logic.handle(command(Command::Kind::Flush), source).status, kStatusInvalidOperation);
    EXPECT_EQ(logic.handle(command(Command::Kind::Unsupported), source).status, kStatusInvalidOperation);
    EXPECT_EQ(logic.handle(command(Command::Kind::Burst, -1), source).status, kStatusBadValue);
    EXPECT_TRUE(logic.state() == StreamState::Standby);
    EXPECT_TRUE(sink.commands.empty());
}

TEST(pr4_a_short_read_sends_only_whole_frames) {
    FakeSink sink;
    FakeClock clock;
    FakeSource source;
    source.available = 10;  // two frames and two stray bytes
    OutputStreamLogic logic(sink, clock, format());
    logic.handle(command(Command::Kind::Start), source);
    const Reply r = logic.handle(command(Command::Kind::Burst, kBurstBytes), source);
    EXPECT_EQ(r.fmqByteCount, 8);
    EXPECT_EQ(sink.sent.size(), 1u);
    EXPECT_EQ(sink.sent[0].bytes, 8u);
}

int main() { return dsa::test::runAll(); }
