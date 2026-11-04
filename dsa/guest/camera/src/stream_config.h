// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// What each host camera offers Android, worked out from the modes the host lists: the
// stream sizes for preview, video and stills, their frame durations, the frame rate ranges,
// and whether a session's streams can run together. Nothing here talks to Android or the
// host, so the rules are tested on the build machine.

#pragma once

#include <cstdint>
#include <map>
#include <string>
#include <utility>
#include <vector>

#include "camera_messages.h"

namespace dsa::camera {

// The pixel formats the HAL offers, as android.hardware.graphics.common.PixelFormat numbers
// them.
constexpr int32_t kFormatBlob = 0x21;                   // JPEG stills
constexpr int32_t kFormatImplementationDefined = 0x22;  // preview and video, the HAL's choice
constexpr int32_t kFormatYcbcr420 = 0x23;               // apps that read the pictures

// The most streams one session takes: two processed (preview, video or YUV) and one still.
constexpr int kMaxProcessedStreams = 2;
constexpr int kMaxStillStreams = 1;

struct StreamSize {
    uint32_t width = 0;
    uint32_t height = 0;
    // The shortest time between two pictures of this size, in nanoseconds.
    int64_t minFrameDurationNs = 0;
};

// The sizes a camera offers, largest first: each of its modes.
std::vector<StreamSize> streamSizes(const HostCamera& camera);

// The frame rate ranges (lowest, highest) a camera offers: every fixed rate its modes reach,
// and each mode's own range, without repeats, in increasing order.
std::vector<std::pair<int32_t, int32_t>> fpsRanges(const HostCamera& camera);

// The frame rate range a request template starts with: up to 30 frames a second when the
// camera has that, fixed (`steady`) for video, else as wide as it goes, so the picture stays
// bright in a dim room.
std::pair<int32_t, int32_t> targetFpsRange(const HostCamera& camera, bool steady);

// The largest size a camera offers: the size of its sensor, as Android sees it.
StreamSize largest(const HostCamera& camera);

// One stream a session asks for.
struct StreamAsk {
    int32_t format = 0;
    uint32_t width = 0;
    uint32_t height = 0;
    bool output = true;
};

// Whether the streams can run together on `camera`; `why` says what is wrong when not.
bool streamsSupported(const HostCamera& camera, const std::vector<StreamAsk>& streams, std::string* why);

// The size the host camera runs at for these streams: the largest of the preview and video
// streams, or the still's when there is no other. Every stream is filled from it.
StreamSize hostSizeFor(const HostCamera& camera, const std::vector<StreamAsk>& streams);

// Android's camera IDs ("0", "1", ...), one for each host camera and kept while the HAL runs,
// so a camera unplugged and plugged in again has the ID it had.
class CameraIds {
   public:
    std::string idFor(const std::string& hostId);
    // The host camera an Android ID stands for; empty when none.
    std::string hostIdOf(const std::string& id) const;

   private:
    std::map<std::string, std::string> mByHost;
    int mNext = 0;
};

}  // namespace dsa::camera
