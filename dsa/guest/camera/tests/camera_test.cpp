// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The camera HAL's rules: its camera port messages against the checked-in protocol vectors,
// so they stay in step with the host's; what a host camera offers Android (sizes, rates and
// the streams that run together); the copy of a picture into a buffer at the buffer's size;
// and the JPEG trailer Android reads a still's length from. The protocol folder is the only
// argument.

#include <cstdio>
#include <cstring>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

#include "camera_messages.h"
#include "frame_copy.h"
#include "stream_config.h"
#include "test_main.h"

using namespace dsa::camera;

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

HostCamera webcam() {
    HostCamera camera;
    camera.id = "front";
    camera.name = "Integrated Camera";
    camera.facing = Facing::Front;
    camera.modes = {Mode{640, 480, 30, 15}, Mode{1280, 720, 30, 5}, Mode{1280, 720, 60, 60}};
    return camera;
}

}  // namespace

TEST(fr26_the_open_request_encodes_as_the_protocol_vector) {
    EXPECT_EQ(encodeOpen("front", 1280, 720, 30), vector("camera_open"));
}

TEST(fr26_the_still_request_encodes_as_the_protocol_vector) {
    EXPECT_EQ(encodeStill(4, 1920, 1080, 95), vector("camera_still"));
}

TEST(fr26_the_hosts_camera_list_decodes_with_facing_and_modes) {
    const std::vector<uint8_t> bytes = vector("camera_list");
    const HostMessage message = decodeHostFrame(bytes.data(), bytes.size());
    const auto* list = std::get_if<CameraList>(&message);
    EXPECT_TRUE(list != nullptr);
    if (list && list->cameras.size() == 1) {
        const HostCamera& camera = list->cameras[0];
        EXPECT_EQ(camera.id, std::string("front"));
        EXPECT_EQ(camera.name, std::string("Integrated Camera"));
        EXPECT_TRUE(camera.facing == Facing::Front);
        EXPECT_EQ(camera.sensorOrientation, 90u);
        EXPECT_EQ(camera.modes.size(), 1u);
        EXPECT_EQ(camera.modes[0].width, 1280u);
        EXPECT_EQ(camera.modes[0].height, 720u);
        EXPECT_EQ(camera.modes[0].maxFps, 30u);
        EXPECT_EQ(camera.modes[0].minFps, 15u);
    } else {
        ::dsa::test::fail(__FILE__, __LINE__, "expected one camera");
    }
}

TEST(fr26_a_host_picture_decodes) {
    const std::vector<uint8_t> bytes = vector("camera_frame");
    const HostMessage message = decodeHostFrame(bytes.data(), bytes.size());
    const auto* picture = std::get_if<Picture>(&message);
    EXPECT_TRUE(picture != nullptr);
    if (picture) {
        EXPECT_EQ(picture->sequence, 7u);
        EXPECT_EQ(picture->timestampNs, 33333333u);
        EXPECT_EQ(picture->width, 2u);
        EXPECT_EQ(picture->height, 2u);
        EXPECT_EQ(picture->stride, 2u);
        EXPECT_EQ(picture->nv12, std::string("\x10\x10\x10\x10\x80\x80", 6));
    }
}

TEST(fr26_a_camera_offers_each_size_once_largest_first) {
    const std::vector<StreamSize> sizes = streamSizes(webcam());
    EXPECT_EQ(sizes.size(), 2u);
    if (sizes.size() == 2) {
        EXPECT_EQ(sizes[0].width, 1280u);
        // Its fastest mode sets its shortest frame time.
        EXPECT_EQ(sizes[0].minFrameDurationNs, 1000000000LL / 60);
        EXPECT_EQ(sizes[1].width, 640u);
        EXPECT_EQ(sizes[1].minFrameDurationNs, 1000000000LL / 30);
    }
    const auto ranges = fpsRanges(webcam());
    const std::vector<std::pair<int32_t, int32_t>> expected = {{5, 30}, {15, 30}, {30, 30}, {60, 60}};
    EXPECT_EQ(ranges, expected);
}

TEST(fr26_preview_and_video_start_at_up_to_30_frames) {
    EXPECT_EQ(targetFpsRange(webcam(), false), (std::pair<int32_t, int32_t>{5, 30}));
    EXPECT_EQ(targetFpsRange(webcam(), true), (std::pair<int32_t, int32_t>{30, 30}));
}

TEST(fr26_preview_video_and_still_streams_run_together) {
    std::string why;
    EXPECT_TRUE(streamsSupported(webcam(),
                                 {{kFormatImplementationDefined, 1280, 720, true},
                                  {kFormatYcbcr420, 640, 480, true},
                                  {kFormatBlob, 1280, 720, true}},
                                 &why));
    // A size the camera does not have, an input stream, or a third picture stream do not.
    EXPECT_TRUE(!streamsSupported(webcam(), {{kFormatYcbcr420, 1920, 1080, true}}, &why));
    EXPECT_TRUE(!streamsSupported(webcam(), {{kFormatYcbcr420, 640, 480, false}}, &why));
    EXPECT_TRUE(!streamsSupported(webcam(),
                                  {{kFormatYcbcr420, 640, 480, true},
                                   {kFormatYcbcr420, 640, 480, true},
                                   {kFormatImplementationDefined, 640, 480, true}},
                                  &why));
    // The host runs the largest picture stream; a still alone runs at its own size.
    const StreamSize size =
        hostSizeFor(webcam(), {{kFormatYcbcr420, 640, 480, true}, {kFormatBlob, 1280, 720, true}});
    EXPECT_EQ(size.width, 640u);
    const StreamSize still = hostSizeFor(webcam(), {{kFormatBlob, 1280, 720, true}});
    EXPECT_EQ(still.width, 1280u);
}

TEST(fr26_android_camera_ids_stay_with_their_host_camera) {
    CameraIds ids;
    EXPECT_EQ(ids.idFor("front"), std::string("0"));
    EXPECT_EQ(ids.idFor("back"), std::string("1"));
    EXPECT_EQ(ids.idFor("front"), std::string("0"));
    EXPECT_EQ(ids.hostIdOf("1"), std::string("back"));
    EXPECT_EQ(ids.hostIdOf("7"), std::string());
}

TEST(fr26_a_picture_scales_into_a_smaller_yuv_buffer) {
    // 4x2: luma 1, 2, 3, 4 in each row; chroma pairs (20, 30) and (21, 31).
    const uint8_t data[] = {1, 2, 3, 4, 1, 2, 3, 4, 20, 30, 21, 31};
    const Nv12 picture{data, sizeof(data), 4, 2, 4};
    std::vector<uint8_t> y(2 * 2, 0), u(1, 0), v(1, 0);
    YuvPlanes planes{{y.data(), 1, 2}, {u.data(), 1, 1}, {v.data(), 1, 1}};
    EXPECT_TRUE(scaleToYuv(picture, 2, 2, planes));
    EXPECT_EQ(y, (std::vector<uint8_t>{1, 3, 1, 3}));
    EXPECT_EQ(u[0], 20);
    EXPECT_EQ(v[0], 30);
}

TEST(fr26_a_picture_of_its_own_size_copies_into_interleaved_chroma) {
    const uint8_t data[] = {1, 2, 3, 4, 5, 6, 7, 8, 20, 30, 21, 31};
    const Nv12 picture{data, sizeof(data), 4, 2, 4};
    std::vector<uint8_t> y(4 * 2, 0), uv(4, 0);
    YuvPlanes planes{{y.data(), 1, 4}, {uv.data(), 2, 4}, {uv.data() + 1, 2, 4}};
    EXPECT_TRUE(scaleToYuv(picture, 4, 2, planes));
    EXPECT_EQ(y, (std::vector<uint8_t>{1, 2, 3, 4, 5, 6, 7, 8}));
    EXPECT_EQ(uv, (std::vector<uint8_t>{20, 30, 21, 31}));
}

TEST(fr26_a_short_picture_is_not_copied) {
    const uint8_t data[] = {1, 2, 3};
    const Nv12 picture{data, sizeof(data), 4, 2, 4};
    std::vector<uint8_t> y(8, 0), u(2, 0), v(2, 0);
    YuvPlanes planes{{y.data(), 1, 4}, {u.data(), 1, 2}, {v.data(), 1, 2}};
    EXPECT_TRUE(!scaleToYuv(picture, 4, 2, planes));
    EXPECT_EQ(y, std::vector<uint8_t>(8, 0));
}

TEST(fr26_a_grey_picture_stays_grey_in_rgba) {
    const uint8_t data[] = {126, 126, 126, 126, 128, 128};
    const Nv12 picture{data, sizeof(data), 2, 2, 2};
    std::vector<uint8_t> rgba(2 * 2 * 4, 0);
    EXPECT_TRUE(scaleToRgba(picture, 2, 2, rgba.data(), 8));
    for (size_t i = 0; i < rgba.size(); ++i) {
        if (i % 4 == 3) {
            EXPECT_EQ(rgba[i], 255);
        } else {
            EXPECT_TRUE(rgba[i] >= 127 && rgba[i] <= 130);
        }
    }
}

TEST(fr26_a_still_carries_its_length_at_the_buffer_end) {
    const uint8_t jpeg[] = {0xFF, 0xD8, 0xFF, 0xD9};
    std::vector<uint8_t> buffer(32, 0);
    EXPECT_TRUE(writeJpegBlob(buffer.data(), buffer.size(), jpeg, sizeof(jpeg)));
    EXPECT_EQ(std::vector<uint8_t>(buffer.begin(), buffer.begin() + 4), (std::vector<uint8_t>{0xFF, 0xD8, 0xFF, 0xD9}));
    int32_t trailer[2] = {0, 0};
    std::memcpy(trailer, buffer.data() + buffer.size() - sizeof(trailer), sizeof(trailer));
    EXPECT_EQ(trailer[0], 0x00FF);
    EXPECT_EQ(trailer[1], 4);
    std::vector<uint8_t> small(10, 0);
    EXPECT_TRUE(!writeJpegBlob(small.data(), small.size(), jpeg, sizeof(jpeg)));
}

TEST(fr26_a_frame_has_its_length_in_front) {
    EXPECT_EQ(withLength({1, 2, 3}), (std::vector<uint8_t>{3, 0, 0, 0, 1, 2, 3}));
}

int main(int argc, char** argv) {
    if (argc != 2) {
        std::fprintf(stderr, "usage: %s <dsa/protocol>\n", argv[0]);
        return 2;
    }
    gProtocolDir = argv[1];
    return ::dsa::test::runAll();
}
