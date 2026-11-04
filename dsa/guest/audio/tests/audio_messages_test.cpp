// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The audio port's messages, through the generated codec: the HAL says it is the guest's
// audio HAL with this build's schema, the stream's format is the one the host plays, and the
// host's answers are understood.

#include <string>
#include <variant>

#include "audio_messages.h"
#include "control.pb.h"
#include "control_schema.h"
#include "test_main.h"

using namespace dsa::audio;
namespace v1 = ::dsa::control::v1;

namespace {

v1::Envelope parse(const std::vector<uint8_t>& bytes) {
    v1::Envelope envelope;
    EXPECT_TRUE(envelope.ParseFromArray(bytes.data(), static_cast<int>(bytes.size())));
    return envelope;
}

std::vector<uint8_t> serialize(const v1::Envelope& envelope) {
    std::string bytes;
    envelope.SerializeToString(&bytes);
    return std::vector<uint8_t>(bytes.begin(), bytes.end());
}

}  // namespace

TEST(pr4_the_hello_names_the_guest_audio_role_and_this_builds_schema) {
    const v1::Envelope envelope = parse(encodeHello("dsa-audio"));
    EXPECT_TRUE(envelope.has_hello());
    EXPECT_EQ(envelope.hello().role(), v1::ROLE_GUEST_AUDIO);
    EXPECT_EQ(envelope.hello().protocol_version(), static_cast<uint32_t>(v1::PROTOCOL_VERSION_CURRENT));
    EXPECT_EQ(envelope.hello().schema_hash().size(), 32u);
    std::string hex;
    for (unsigned char c : envelope.hello().schema_hash()) {
        static const char digits[] = "0123456789abcdef";
        hex.push_back(digits[c >> 4]);
        hex.push_back(digits[c & 15]);
    }
    EXPECT_EQ(hex, std::string(v1::kSchemaHashHex));
}

TEST(pr4_the_stream_opens_at_48_khz_stereo_16_bit) {
    const v1::Envelope envelope = parse(encodeOpen("primary", 960));
    EXPECT_TRUE(envelope.has_audio_output_open());
    EXPECT_EQ(envelope.audio_output_open().sample_rate(), 48000u);
    EXPECT_EQ(envelope.audio_output_open().channel_count(), 2u);
    EXPECT_EQ(envelope.audio_output_open().format(), v1::AUDIO_SAMPLE_FORMAT_S16LE);
    EXPECT_EQ(envelope.audio_output_open().frames_per_burst(), 960u);
    EXPECT_EQ(envelope.audio_output_open().name(), std::string("primary"));
}

TEST(pr4_sound_carries_its_position) {
    const uint8_t data[8] = {1, 2, 3, 4, 5, 6, 7, 8};
    const v1::Envelope envelope = parse(encodePcm(4800, data, sizeof(data)));
    EXPECT_EQ(envelope.audio_pcm().position(), 4800u);
    EXPECT_EQ(envelope.audio_pcm().data().size(), 8u);
}

TEST(pr4_each_state_command_has_its_own_value) {
    EXPECT_EQ(parse(encodeControl(HostCommand::Start)).audio_output_control().command(),
              v1::AUDIO_OUTPUT_COMMAND_START);
    EXPECT_EQ(parse(encodeControl(HostCommand::Pause)).audio_output_control().command(),
              v1::AUDIO_OUTPUT_COMMAND_PAUSE);
    EXPECT_EQ(parse(encodeControl(HostCommand::Flush)).audio_output_control().command(),
              v1::AUDIO_OUTPUT_COMMAND_FLUSH);
    EXPECT_EQ(parse(encodeControl(HostCommand::Standby)).audio_output_control().command(),
              v1::AUDIO_OUTPUT_COMMAND_STANDBY);
}

TEST(pr4_the_hosts_answers_are_understood) {
    v1::Envelope accepted;
    accepted.mutable_hello_accepted();
    const auto acceptedBytes = serialize(accepted);
    EXPECT_TRUE(std::holds_alternative<Accepted>(decodeHostFrame(acceptedBytes.data(), acceptedBytes.size())));

    v1::Envelope rejected;
    rejected.mutable_hello_rejected()->set_detail("wrong schema");
    const auto rejectedBytes = serialize(rejected);
    const HostMessage r = decodeHostFrame(rejectedBytes.data(), rejectedBytes.size());
    EXPECT_TRUE(std::holds_alternative<Rejected>(r));
    EXPECT_TRUE(std::holds_alternative<Rejected>(r) && std::get<Rejected>(r).detail == "wrong schema");

    v1::Envelope ready;
    ready.mutable_audio_output_ready()->set_buffered_frames(1920);
    ready.mutable_audio_output_ready()->set_device_frames(480);
    const auto readyBytes = serialize(ready);
    const HostMessage m = decodeHostFrame(readyBytes.data(), readyBytes.size());
    EXPECT_TRUE(std::holds_alternative<Ready>(m));
    EXPECT_TRUE(std::holds_alternative<Ready>(m) && std::get<Ready>(m).bufferedFrames == 1920 &&
                std::get<Ready>(m).deviceFrames == 480);

    const uint8_t garbage[3] = {0xff, 0xff, 0xff};
    EXPECT_TRUE(std::holds_alternative<std::monostate>(decodeHostFrame(garbage, sizeof(garbage))));
}

TEST(pr4_frames_go_after_their_little_endian_length) {
    const std::vector<uint8_t> framed = withLength(std::vector<uint8_t>(258, 7));
    EXPECT_EQ(framed.size(), 262u);
    EXPECT_EQ(framed[0], 2);
    EXPECT_EQ(framed[1], 1);
    EXPECT_EQ(framed[2], 0);
    EXPECT_EQ(framed[3], 0);
}

int main() { return dsa::test::runAll(); }
