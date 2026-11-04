// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "stream_config.h"

#include <algorithm>
#include <set>

namespace dsa::camera {

namespace {

constexpr int64_t kSecondNs = 1000000000;

uint64_t area(uint32_t width, uint32_t height) { return static_cast<uint64_t>(width) * height; }

const StreamSize* find(const std::vector<StreamSize>& sizes, uint32_t width, uint32_t height) {
    for (const StreamSize& size : sizes) {
        if (size.width == width && size.height == height) {
            return &size;
        }
    }
    return nullptr;
}

}  // namespace

std::vector<StreamSize> streamSizes(const HostCamera& camera) {
    std::vector<StreamSize> sizes;
    for (const Mode& mode : camera.modes) {
        if (mode.width == 0 || mode.height == 0 || mode.maxFps == 0) {
            continue;
        }
        const int64_t duration = kSecondNs / mode.maxFps;
        auto known = std::find_if(sizes.begin(), sizes.end(), [&](const StreamSize& s) {
            return s.width == mode.width && s.height == mode.height;
        });
        if (known != sizes.end()) {
            known->minFrameDurationNs = std::min(known->minFrameDurationNs, duration);
            continue;
        }
        sizes.push_back(StreamSize{mode.width, mode.height, duration});
    }
    std::sort(sizes.begin(), sizes.end(), [](const StreamSize& a, const StreamSize& b) {
        if (area(a.width, a.height) != area(b.width, b.height)) {
            return area(a.width, a.height) > area(b.width, b.height);
        }
        return a.width > b.width;
    });
    return sizes;
}

std::vector<std::pair<int32_t, int32_t>> fpsRanges(const HostCamera& camera) {
    std::set<std::pair<int32_t, int32_t>> ranges;
    for (const Mode& mode : camera.modes) {
        if (mode.maxFps == 0) {
            continue;
        }
        const int32_t high = static_cast<int32_t>(mode.maxFps);
        const int32_t low = static_cast<int32_t>(std::min(mode.minFps == 0 ? mode.maxFps : mode.minFps, mode.maxFps));
        ranges.insert({high, high});
        ranges.insert({low, high});
    }
    return std::vector<std::pair<int32_t, int32_t>>(ranges.begin(), ranges.end());
}

std::pair<int32_t, int32_t> targetFpsRange(const HostCamera& camera, bool steady) {
    std::vector<std::pair<int32_t, int32_t>> ranges = fpsRanges(camera);
    if (ranges.empty()) {
        return {15, 30};
    }
    std::vector<std::pair<int32_t, int32_t>> upTo30;
    for (const auto& range : ranges) {
        if (range.second <= 30) {
            upTo30.push_back(range);
        }
    }
    if (!upTo30.empty()) {
        ranges = upTo30;
    }
    // Ranked by the highest rate, then: for video a fixed rate, else the lowest floor.
    const auto better = [steady](const std::pair<int32_t, int32_t>& a, const std::pair<int32_t, int32_t>& b) {
        if (a.second != b.second) {
            return a.second > b.second;
        }
        if (steady) {
            return a.first > b.first;
        }
        return a.first < b.first;
    };
    std::pair<int32_t, int32_t> best = ranges.front();
    for (const auto& range : ranges) {
        if (better(range, best)) {
            best = range;
        }
    }
    return best;
}

StreamSize largest(const HostCamera& camera) {
    const std::vector<StreamSize> sizes = streamSizes(camera);
    return sizes.empty() ? StreamSize{} : sizes.front();
}

bool streamsSupported(const HostCamera& camera, const std::vector<StreamAsk>& streams, std::string* why) {
    const std::vector<StreamSize> sizes = streamSizes(camera);
    int processed = 0;
    int stills = 0;
    if (streams.empty()) {
        *why = "no stream";
        return false;
    }
    for (const StreamAsk& stream : streams) {
        if (!stream.output) {
            *why = "the camera takes no input stream";
            return false;
        }
        if (find(sizes, stream.width, stream.height) == nullptr) {
            *why = "the camera has no " + std::to_string(stream.width) + "x" + std::to_string(stream.height) + " mode";
            return false;
        }
        switch (stream.format) {
            case kFormatBlob:
                ++stills;
                break;
            case kFormatImplementationDefined:
            case kFormatYcbcr420:
                ++processed;
                break;
            default:
                *why = "the camera has no pixel format " + std::to_string(stream.format);
                return false;
        }
    }
    if (processed > kMaxProcessedStreams || stills > kMaxStillStreams) {
        *why = "too many streams";
        return false;
    }
    return true;
}

StreamSize hostSizeFor(const HostCamera& camera, const std::vector<StreamAsk>& streams) {
    const std::vector<StreamSize> sizes = streamSizes(camera);
    const StreamSize* best = nullptr;
    const StreamSize* still = nullptr;
    for (const StreamAsk& stream : streams) {
        const StreamSize* size = find(sizes, stream.width, stream.height);
        if (size == nullptr) {
            continue;
        }
        const StreamSize** slot = stream.format == kFormatBlob ? &still : &best;
        if (*slot == nullptr || area(size->width, size->height) > area((*slot)->width, (*slot)->height)) {
            *slot = size;
        }
    }
    if (best != nullptr) {
        return *best;
    }
    if (still != nullptr) {
        return *still;
    }
    return largest(camera);
}

std::string CameraIds::idFor(const std::string& hostId) {
    auto known = mByHost.find(hostId);
    if (known != mByHost.end()) {
        return known->second;
    }
    const std::string id = std::to_string(mNext++);
    mByHost.emplace(hostId, id);
    return id;
}

std::string CameraIds::hostIdOf(const std::string& id) const {
    for (const auto& [host, android] : mByHost) {
        if (android == id) {
            return host;
        }
    }
    return std::string();
}

}  // namespace dsa::camera
