// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "audio_messages.h"

#include <cstdlib>

#include "control.pb.h"
#include "control_schema.h"

namespace dsa::audio {

namespace v1 = ::dsa::control::v1;

namespace {

std::vector<uint8_t> encode(const v1::Envelope& envelope) {
    std::string bytes;
    envelope.SerializeToString(&bytes);
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

}  // namespace

std::vector<uint8_t> encodeHello(const std::string& build) {
    v1::Envelope envelope;
    v1::Hello* hello = envelope.mutable_hello();
    hello->set_protocol_version(v1::PROTOCOL_VERSION_CURRENT);
    hello->set_schema_hash(schemaHash());
    hello->set_role(v1::ROLE_GUEST_AUDIO);
    hello->set_build(build);
    return encode(envelope);
}

std::vector<uint8_t> encodeOpen(const std::string& name, uint32_t framesPerBurst) {
    v1::Envelope envelope;
    v1::AudioOutputOpen* open = envelope.mutable_audio_output_open();
    open->set_sample_rate(48000);
    open->set_channel_count(2);
    open->set_format(v1::AUDIO_SAMPLE_FORMAT_S16LE);
    open->set_frames_per_burst(framesPerBurst);
    open->set_name(name);
    return encode(envelope);
}

std::vector<uint8_t> encodePcm(uint64_t position, const uint8_t* data, size_t size) {
    v1::Envelope envelope;
    v1::AudioPcm* pcm = envelope.mutable_audio_pcm();
    pcm->set_position(position);
    pcm->set_data(data, size);
    return encode(envelope);
}

std::vector<uint8_t> encodeControl(HostCommand command) {
    v1::Envelope envelope;
    v1::AudioOutputCommand value = v1::AUDIO_OUTPUT_COMMAND_UNSPECIFIED;
    switch (command) {
        case HostCommand::Start:
            value = v1::AUDIO_OUTPUT_COMMAND_START;
            break;
        case HostCommand::Pause:
            value = v1::AUDIO_OUTPUT_COMMAND_PAUSE;
            break;
        case HostCommand::Flush:
            value = v1::AUDIO_OUTPUT_COMMAND_FLUSH;
            break;
        case HostCommand::Standby:
            value = v1::AUDIO_OUTPUT_COMMAND_STANDBY;
            break;
    }
    envelope.mutable_audio_output_control()->set_command(value);
    return encode(envelope);
}

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
        case v1::Envelope::kAudioOutputReady:
            return Ready{envelope.audio_output_ready().buffered_frames(),
                         envelope.audio_output_ready().device_frames()};
        default:
            return std::monostate{};
    }
}

std::vector<uint8_t> withLength(const std::vector<uint8_t>& frame) {
    std::vector<uint8_t> out;
    out.reserve(frame.size() + 4);
    const uint32_t size = static_cast<uint32_t>(frame.size());
    for (int i = 0; i < 4; ++i) {
        out.push_back(static_cast<uint8_t>(size >> (8 * i)));
    }
    out.insert(out.end(), frame.begin(), frame.end());
    return out;
}

}  // namespace dsa::audio
