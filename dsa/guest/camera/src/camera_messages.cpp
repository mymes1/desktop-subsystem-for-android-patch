// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "camera_messages.h"

#include <cstdlib>

#include "control.pb.h"
#include "control_schema.h"

namespace dsa::camera {

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

Facing facingOf(v1::CameraFacing facing) {
    switch (facing) {
        case v1::CAMERA_FACING_FRONT:
            return Facing::Front;
        case v1::CAMERA_FACING_BACK:
            return Facing::Back;
        default:
            return Facing::External;
    }
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
        case v1::Envelope::kCameraList: {
            CameraList list;
            for (const v1::CameraDevice& device : envelope.camera_list().cameras()) {
                HostCamera camera;
                camera.id = device.id();
                camera.name = device.name();
                camera.facing = facingOf(device.facing());
                camera.sensorOrientation = device.sensor_orientation() % 360;
                for (const v1::CameraMode& mode : device.modes()) {
                    if (mode.width() == 0 || mode.height() == 0 || mode.max_fps() == 0) {
                        continue;
                    }
                    camera.modes.push_back(Mode{mode.width(), mode.height(), mode.max_fps(),
                                                mode.min_fps() == 0 ? mode.max_fps() : mode.min_fps()});
                }
                if (!camera.id.empty() && !camera.modes.empty()) {
                    list.cameras.push_back(std::move(camera));
                }
            }
            return list;
        }
        case v1::Envelope::kCameraOpened: {
            const v1::CameraOpened& opened = envelope.camera_opened();
            return Opened{opened.ok(), opened.error(), opened.width(), opened.height(), opened.fps()};
        }
        case v1::Envelope::kCameraFrame: {
            const v1::CameraFrame& f = envelope.camera_frame();
            Picture picture;
            picture.sequence = f.sequence();
            picture.timestampNs = f.timestamp_ns();
            picture.width = f.width();
            picture.height = f.height();
            picture.stride = f.stride();
            picture.nv12 = f.nv12();
            return picture;
        }
        case v1::Envelope::kCameraStillTaken: {
            const v1::CameraStillTaken& taken = envelope.camera_still_taken();
            return StillTaken{taken.request_id(), taken.jpeg(), taken.error()};
        }
        case v1::Envelope::kCameraError:
            return CameraFailed{envelope.camera_error().error()};
        default:
            return std::monostate{};
    }
}

std::vector<uint8_t> encodeHello(const std::string& build) {
    v1::Envelope envelope;
    v1::Hello* hello = envelope.mutable_hello();
    hello->set_protocol_version(v1::PROTOCOL_VERSION_CURRENT);
    hello->set_schema_hash(schemaHash());
    hello->set_role(v1::ROLE_GUEST_CAMERA);
    hello->set_build(build);
    return encode(envelope);
}

std::vector<uint8_t> encodeOpen(const std::string& id, uint32_t width, uint32_t height, uint32_t fps) {
    v1::Envelope envelope;
    v1::CameraOpen* open = envelope.mutable_camera_open();
    open->set_id(id);
    open->set_width(width);
    open->set_height(height);
    open->set_fps(fps);
    return encode(envelope);
}

std::vector<uint8_t> encodeStop() {
    v1::Envelope envelope;
    envelope.mutable_camera_stop();
    return encode(envelope);
}

std::vector<uint8_t> encodeStill(uint32_t requestId, uint32_t width, uint32_t height, uint32_t quality) {
    v1::Envelope envelope;
    v1::CameraStill* still = envelope.mutable_camera_still();
    still->set_request_id(requestId);
    still->set_width(width);
    still->set_height(height);
    still->set_quality(quality);
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

}  // namespace dsa::camera
