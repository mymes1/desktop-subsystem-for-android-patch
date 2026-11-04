// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The input stream's connection to the host, against a fake host on a socket pair: the open
// fails with the host's reason when the host does not record, the host's sound reaches the
// stream, and closing the stream closes the connection, so the host closes its device.

#include <sys/socket.h>
#include <unistd.h>

#include <chrono>
#include <string>
#include <thread>
#include <vector>

#include "audio_messages.h"
#include "control.pb.h"
#include "input_link.h"
#include "socket_io.h"
#include "test_main.h"

using namespace dsa::audio;
namespace v1 = ::dsa::control::v1;

namespace {

std::vector<uint8_t> serialize(const v1::Envelope& envelope) {
    std::string bytes;
    envelope.SerializeToString(&bytes);
    return withLength(std::vector<uint8_t>(bytes.begin(), bytes.end()));
}

v1::Envelope readEnvelope(int fd) {
    std::vector<uint8_t> frame;
    v1::Envelope envelope;
    if (readFrame(fd, &frame)) {
        envelope.ParseFromArray(frame.data(), static_cast<int>(frame.size()));
    }
    return envelope;
}

// A host that accepts the hello and answers the open with `error` (empty: it records).
struct FakeHost {
    int guest = -1;
    int host = -1;
    bool sawInputOpen = false;
    std::thread thread;

    explicit FakeHost(std::string error) {
        int fds[2];
        socketpair(AF_UNIX, SOCK_STREAM, 0, fds);
        guest = fds[0];
        host = fds[1];
        thread = std::thread([this, error] {
            if (!readEnvelope(host).has_hello()) return;
            v1::Envelope accepted;
            accepted.mutable_hello_accepted();
            writeAll(host, serialize(accepted));
            sawInputOpen = readEnvelope(host).has_audio_input_open();
            v1::Envelope opened;
            opened.mutable_audio_input_opened()->set_error(error);
            opened.mutable_audio_input_opened()->set_device_frames(480);
            writeAll(host, serialize(opened));
        });
    }
    ~FakeHost() {
        if (thread.joinable()) thread.join();
        close(host);
    }

    void join() {
        if (thread.joinable()) thread.join();
    }

    void send(uint64_t position, size_t frames, char value) {
        v1::Envelope pcm;
        pcm.mutable_audio_pcm()->set_position(position);
        pcm.mutable_audio_pcm()->set_data(std::string(frames * 4, value));
        writeAll(host, serialize(pcm));
    }
};

}  // namespace

TEST(fr26_a_host_that_does_not_record_fails_the_open_with_its_reason) {
    FakeHost host("Windows lets no desktop app use the microphone");
    std::string error;
    const int fd = host.guest;
    auto link = InputLink::open("primary", 960, [fd] { return fd; }, &error);
    host.join();
    EXPECT_TRUE(link == nullptr);
    EXPECT_TRUE(host.sawInputOpen);
    EXPECT_EQ(error, std::string("Windows lets no desktop app use the microphone"));
}

TEST(fr26_no_host_fails_the_open) {
    std::string error;
    auto link = InputLink::open("primary", 960, [] { return -1; }, &error);
    EXPECT_TRUE(link == nullptr);
    EXPECT_TRUE(!error.empty());
}

TEST(fr26_the_hosts_recorded_sound_reaches_the_stream) {
    FakeHost host("");
    std::string error;
    const int fd = host.guest;
    auto link = InputLink::open("primary", 960, [fd] { return fd; }, &error);
    host.join();
    EXPECT_TRUE(link != nullptr);
    if (link == nullptr) return;
    EXPECT_TRUE(link->hostDelayFrames() >= 480u);
    host.send(0, 480, 4);
    host.send(480, 480, 4);
    std::vector<uint8_t> out(480 * 4);
    size_t sound = 0;
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (sound == 0 && std::chrono::steady_clock::now() < deadline) {
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
        sound = link->pull(out.data(), 480);
    }
    EXPECT_EQ(sound, 480u);
    EXPECT_EQ(out[0], 4);
}

TEST(fr26_closing_the_stream_closes_the_hosts_connection) {
    FakeHost host("");
    std::string error;
    const int fd = host.guest;
    auto link = InputLink::open("primary", 960, [fd] { return fd; }, &error);
    host.join();
    EXPECT_TRUE(link != nullptr);
    link.reset();
    // The host sees the connection end, which is when it closes its capture device.
    uint8_t byte = 0;
    EXPECT_EQ(recv(host.host, &byte, 1, 0), 0);
}

int main() { return dsa::test::runAll(); }
