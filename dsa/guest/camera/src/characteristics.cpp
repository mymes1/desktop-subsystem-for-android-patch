// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "characteristics.h"

#include <algorithm>

#include "stream_config.h"

namespace dsa::camera {

namespace {

// A webcam's lens: about 3 mm, on a sensor of about 3.6 by 2.7 mm.
constexpr float kFocalLengthMm = 3.0f;
constexpr float kSensorWidthMm = 3.6f;

// The tags each capture result adds to its settings.
const std::vector<int32_t>& resultTags() {
    static const std::vector<int32_t> tags = {
        ANDROID_SENSOR_TIMESTAMP,      ANDROID_SENSOR_FRAME_DURATION, ANDROID_CONTROL_AE_STATE,
        ANDROID_CONTROL_AF_STATE,      ANDROID_CONTROL_AWB_STATE,     ANDROID_FLASH_STATE,
        ANDROID_LENS_STATE,            ANDROID_REQUEST_PIPELINE_DEPTH, ANDROID_STATISTICS_SCENE_FLICKER,
    };
    return tags;
}

constexpr uint8_t kPipelineDepth = 4;

}  // namespace

int32_t jpegMaxSize(const HostCamera& camera) {
    const StreamSize size = largest(camera);
    // A camera picture's JPEG stays well under 1.5 bytes a pixel, even at full quality.
    const int64_t bytes = static_cast<int64_t>(size.width) * size.height * 3 / 2 + 64 * 1024;
    return static_cast<int32_t>(std::min<int64_t>(bytes, 32 * 1024 * 1024));
}

Metadata characteristics(const HostCamera& camera) {
    Metadata m;
    uint8_t facing = ANDROID_LENS_FACING_EXTERNAL;
    if (camera.facing == Facing::Front) {
        facing = ANDROID_LENS_FACING_FRONT;
    } else if (camera.facing == Facing::Back) {
        facing = ANDROID_LENS_FACING_BACK;
    }
    m.set(ANDROID_LENS_FACING, std::vector<uint8_t>{facing});
    m.set(ANDROID_SENSOR_ORIENTATION, std::vector<int32_t>{static_cast<int32_t>(camera.sensorOrientation % 360)});
    m.set(ANDROID_INFO_SUPPORTED_HARDWARE_LEVEL,
          std::vector<uint8_t>{ANDROID_INFO_SUPPORTED_HARDWARE_LEVEL_EXTERNAL});
    m.set(ANDROID_REQUEST_AVAILABLE_CAPABILITIES,
          std::vector<uint8_t>{ANDROID_REQUEST_AVAILABLE_CAPABILITIES_BACKWARD_COMPATIBLE});
    m.set(ANDROID_REQUEST_PARTIAL_RESULT_COUNT, std::vector<int32_t>{1});
    m.set(ANDROID_REQUEST_PIPELINE_MAX_DEPTH, std::vector<uint8_t>{kPipelineDepth});
    m.set(ANDROID_REQUEST_MAX_NUM_OUTPUT_STREAMS, std::vector<int32_t>{0, kMaxProcessedStreams, kMaxStillStreams});
    m.set(ANDROID_REQUEST_MAX_NUM_INPUT_STREAMS, std::vector<int32_t>{0});

    const std::vector<StreamSize> sizes = streamSizes(camera);
    std::vector<int32_t> configurations;
    std::vector<int64_t> durations;
    std::vector<int64_t> stalls;
    int64_t slowest = 0;
    for (const StreamSize& size : sizes) {
        for (int32_t format : {kFormatImplementationDefined, kFormatYcbcr420, kFormatBlob}) {
            configurations.insert(configurations.end(),
                                  {format, static_cast<int32_t>(size.width), static_cast<int32_t>(size.height),
                                   ANDROID_SCALER_AVAILABLE_STREAM_CONFIGURATIONS_OUTPUT});
            durations.insert(durations.end(), {format, static_cast<int64_t>(size.width),
                                               static_cast<int64_t>(size.height), size.minFrameDurationNs});
        }
        // A still is encoded on the host from the running picture: about one frame's stall.
        stalls.insert(stalls.end(), {kFormatBlob, static_cast<int64_t>(size.width), static_cast<int64_t>(size.height),
                                     size.minFrameDurationNs});
        slowest = std::max(slowest, size.minFrameDurationNs);
    }
    m.set(ANDROID_SCALER_AVAILABLE_STREAM_CONFIGURATIONS, configurations);
    m.set(ANDROID_SCALER_AVAILABLE_MIN_FRAME_DURATIONS, durations);
    m.set(ANDROID_SCALER_AVAILABLE_STALL_DURATIONS, stalls);
    m.set(ANDROID_SCALER_AVAILABLE_MAX_DIGITAL_ZOOM, std::vector<float>{1.0f});
    m.set(ANDROID_SCALER_CROPPING_TYPE, std::vector<uint8_t>{ANDROID_SCALER_CROPPING_TYPE_CENTER_ONLY});

    const StreamSize full = largest(camera);
    const int32_t width = static_cast<int32_t>(full.width);
    const int32_t height = static_cast<int32_t>(full.height);
    m.set(ANDROID_SENSOR_INFO_ACTIVE_ARRAY_SIZE, std::vector<int32_t>{0, 0, width, height});
    m.set(ANDROID_SENSOR_INFO_PRE_CORRECTION_ACTIVE_ARRAY_SIZE, std::vector<int32_t>{0, 0, width, height});
    m.set(ANDROID_SENSOR_INFO_PIXEL_ARRAY_SIZE, std::vector<int32_t>{width, height});
    const float sensorHeight = width > 0 ? kSensorWidthMm * static_cast<float>(height) / static_cast<float>(width) : 0;
    m.set(ANDROID_SENSOR_INFO_PHYSICAL_SIZE, std::vector<float>{kSensorWidthMm, sensorHeight});
    m.set(ANDROID_SENSOR_INFO_TIMESTAMP_SOURCE, std::vector<uint8_t>{ANDROID_SENSOR_INFO_TIMESTAMP_SOURCE_UNKNOWN});
    m.set(ANDROID_SENSOR_INFO_MAX_FRAME_DURATION, std::vector<int64_t>{std::max<int64_t>(slowest, 1000000000LL)});

    m.set(ANDROID_LENS_INFO_AVAILABLE_FOCAL_LENGTHS, std::vector<float>{kFocalLengthMm});
    m.set(ANDROID_LENS_INFO_MINIMUM_FOCUS_DISTANCE, std::vector<float>{0.0f});
    m.set(ANDROID_LENS_INFO_AVAILABLE_OPTICAL_STABILIZATION,
          std::vector<uint8_t>{ANDROID_LENS_OPTICAL_STABILIZATION_MODE_OFF});

    std::vector<int32_t> ranges;
    for (const auto& [low, high] : fpsRanges(camera)) {
        ranges.push_back(low);
        ranges.push_back(high);
    }
    m.set(ANDROID_CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES, ranges);
    m.set(ANDROID_CONTROL_AE_COMPENSATION_RANGE, std::vector<int32_t>{0, 0});
    m.set(ANDROID_CONTROL_AE_COMPENSATION_STEP, std::vector<camera_metadata_rational_t>{{0, 1}});
    m.set(ANDROID_CONTROL_AE_AVAILABLE_ANTIBANDING_MODES, std::vector<uint8_t>{ANDROID_CONTROL_AE_ANTIBANDING_MODE_AUTO});
    m.set(ANDROID_CONTROL_AE_AVAILABLE_MODES, std::vector<uint8_t>{ANDROID_CONTROL_AE_MODE_ON});
    m.set(ANDROID_CONTROL_AE_LOCK_AVAILABLE, std::vector<uint8_t>{ANDROID_CONTROL_AE_LOCK_AVAILABLE_FALSE});
    m.set(ANDROID_CONTROL_AF_AVAILABLE_MODES, std::vector<uint8_t>{ANDROID_CONTROL_AF_MODE_OFF});
    m.set(ANDROID_CONTROL_AWB_AVAILABLE_MODES, std::vector<uint8_t>{ANDROID_CONTROL_AWB_MODE_AUTO});
    m.set(ANDROID_CONTROL_AWB_LOCK_AVAILABLE, std::vector<uint8_t>{ANDROID_CONTROL_AWB_LOCK_AVAILABLE_FALSE});
    m.set(ANDROID_CONTROL_AVAILABLE_EFFECTS, std::vector<uint8_t>{ANDROID_CONTROL_EFFECT_MODE_OFF});
    m.set(ANDROID_CONTROL_AVAILABLE_SCENE_MODES, std::vector<uint8_t>{ANDROID_CONTROL_SCENE_MODE_DISABLED});
    m.set(ANDROID_CONTROL_AVAILABLE_MODES, std::vector<uint8_t>{ANDROID_CONTROL_MODE_AUTO});
    m.set(ANDROID_CONTROL_AVAILABLE_VIDEO_STABILIZATION_MODES,
          std::vector<uint8_t>{ANDROID_CONTROL_VIDEO_STABILIZATION_MODE_OFF});
    m.set(ANDROID_CONTROL_MAX_REGIONS, std::vector<int32_t>{0, 0, 0});

    // No thumbnails: a still carries the picture only.
    m.set(ANDROID_JPEG_AVAILABLE_THUMBNAIL_SIZES, std::vector<int32_t>{0, 0});
    m.set(ANDROID_JPEG_MAX_SIZE, std::vector<int32_t>{jpegMaxSize(camera)});
    m.set(ANDROID_FLASH_INFO_AVAILABLE, std::vector<uint8_t>{ANDROID_FLASH_INFO_AVAILABLE_FALSE});
    m.set(ANDROID_STATISTICS_INFO_AVAILABLE_FACE_DETECT_MODES,
          std::vector<uint8_t>{ANDROID_STATISTICS_FACE_DETECT_MODE_OFF});
    m.set(ANDROID_STATISTICS_INFO_MAX_FACE_COUNT, std::vector<int32_t>{0});
    m.set(ANDROID_NOISE_REDUCTION_AVAILABLE_NOISE_REDUCTION_MODES,
          std::vector<uint8_t>{ANDROID_NOISE_REDUCTION_MODE_OFF});
    m.set(ANDROID_EDGE_AVAILABLE_EDGE_MODES, std::vector<uint8_t>{ANDROID_EDGE_MODE_OFF});
    m.set(ANDROID_COLOR_CORRECTION_AVAILABLE_ABERRATION_MODES,
          std::vector<uint8_t>{ANDROID_COLOR_CORRECTION_ABERRATION_MODE_OFF});
    m.set(ANDROID_SYNC_MAX_LATENCY, std::vector<int32_t>{ANDROID_SYNC_MAX_LATENCY_UNKNOWN});

    // The keys an app may set, and those a result carries: the template's, and the result's own.
    Metadata settings;
    defaultSettings(camera, /* PREVIEW */ 1, &settings);
    std::vector<int32_t> requestKeys = settings.tags();
    std::vector<int32_t> resultKeys = requestKeys;
    resultKeys.insert(resultKeys.end(), resultTags().begin(), resultTags().end());
    m.set(ANDROID_REQUEST_AVAILABLE_REQUEST_KEYS, requestKeys);
    m.set(ANDROID_REQUEST_AVAILABLE_RESULT_KEYS, resultKeys);
    std::vector<int32_t> characteristicsKeys = m.tags();
    characteristicsKeys.push_back(ANDROID_REQUEST_AVAILABLE_CHARACTERISTICS_KEYS);
    m.set(ANDROID_REQUEST_AVAILABLE_CHARACTERISTICS_KEYS, characteristicsKeys);
    return m;
}

bool defaultSettings(const HostCamera& camera, int32_t templateId, Metadata* settings) {
    uint8_t intent = 0;
    switch (templateId) {
        case 1:
            intent = ANDROID_CONTROL_CAPTURE_INTENT_PREVIEW;
            break;
        case 2:
            intent = ANDROID_CONTROL_CAPTURE_INTENT_STILL_CAPTURE;
            break;
        case 3:
            intent = ANDROID_CONTROL_CAPTURE_INTENT_VIDEO_RECORD;
            break;
        case 4:
            intent = ANDROID_CONTROL_CAPTURE_INTENT_VIDEO_SNAPSHOT;
            break;
        default:
            return false;
    }
    Metadata m;
    // Video wants a steady rate; the others the widest range, so the picture stays bright.
    const auto [low, high] = targetFpsRange(camera, intent == ANDROID_CONTROL_CAPTURE_INTENT_VIDEO_RECORD);
    const StreamSize full = largest(camera);
    m.set(ANDROID_CONTROL_CAPTURE_INTENT, std::vector<uint8_t>{intent});
    m.set(ANDROID_CONTROL_MODE, std::vector<uint8_t>{ANDROID_CONTROL_MODE_AUTO});
    m.set(ANDROID_CONTROL_AE_MODE, std::vector<uint8_t>{ANDROID_CONTROL_AE_MODE_ON});
    m.set(ANDROID_CONTROL_AE_TARGET_FPS_RANGE, std::vector<int32_t>{low, high});
    m.set(ANDROID_CONTROL_AE_ANTIBANDING_MODE, std::vector<uint8_t>{ANDROID_CONTROL_AE_ANTIBANDING_MODE_AUTO});
    m.set(ANDROID_CONTROL_AE_EXPOSURE_COMPENSATION, std::vector<int32_t>{0});
    m.set(ANDROID_CONTROL_AE_LOCK, std::vector<uint8_t>{ANDROID_CONTROL_AE_LOCK_OFF});
    m.set(ANDROID_CONTROL_AE_PRECAPTURE_TRIGGER, std::vector<uint8_t>{ANDROID_CONTROL_AE_PRECAPTURE_TRIGGER_IDLE});
    m.set(ANDROID_CONTROL_AF_MODE, std::vector<uint8_t>{ANDROID_CONTROL_AF_MODE_OFF});
    m.set(ANDROID_CONTROL_AF_TRIGGER, std::vector<uint8_t>{ANDROID_CONTROL_AF_TRIGGER_IDLE});
    m.set(ANDROID_CONTROL_AWB_MODE, std::vector<uint8_t>{ANDROID_CONTROL_AWB_MODE_AUTO});
    m.set(ANDROID_CONTROL_AWB_LOCK, std::vector<uint8_t>{ANDROID_CONTROL_AWB_LOCK_OFF});
    m.set(ANDROID_CONTROL_EFFECT_MODE, std::vector<uint8_t>{ANDROID_CONTROL_EFFECT_MODE_OFF});
    m.set(ANDROID_CONTROL_SCENE_MODE, std::vector<uint8_t>{ANDROID_CONTROL_SCENE_MODE_DISABLED});
    m.set(ANDROID_CONTROL_VIDEO_STABILIZATION_MODE, std::vector<uint8_t>{ANDROID_CONTROL_VIDEO_STABILIZATION_MODE_OFF});
    m.set(ANDROID_FLASH_MODE, std::vector<uint8_t>{ANDROID_FLASH_MODE_OFF});
    m.set(ANDROID_JPEG_QUALITY, std::vector<uint8_t>{95});
    m.set(ANDROID_JPEG_ORIENTATION, std::vector<int32_t>{0});
    m.set(ANDROID_JPEG_THUMBNAIL_SIZE, std::vector<int32_t>{0, 0});
    m.set(ANDROID_JPEG_THUMBNAIL_QUALITY, std::vector<uint8_t>{90});
    m.set(ANDROID_LENS_FOCAL_LENGTH, std::vector<float>{kFocalLengthMm});
    m.set(ANDROID_LENS_OPTICAL_STABILIZATION_MODE, std::vector<uint8_t>{ANDROID_LENS_OPTICAL_STABILIZATION_MODE_OFF});
    m.set(ANDROID_NOISE_REDUCTION_MODE, std::vector<uint8_t>{ANDROID_NOISE_REDUCTION_MODE_OFF});
    m.set(ANDROID_EDGE_MODE, std::vector<uint8_t>{ANDROID_EDGE_MODE_OFF});
    m.set(ANDROID_STATISTICS_FACE_DETECT_MODE, std::vector<uint8_t>{ANDROID_STATISTICS_FACE_DETECT_MODE_OFF});
    m.set(ANDROID_COLOR_CORRECTION_ABERRATION_MODE, std::vector<uint8_t>{ANDROID_COLOR_CORRECTION_ABERRATION_MODE_OFF});
    m.set(ANDROID_SCALER_CROP_REGION,
          std::vector<int32_t>{0, 0, static_cast<int32_t>(full.width), static_cast<int32_t>(full.height)});
    *settings = m;
    return true;
}

Metadata captureResult(const Metadata& settings, int64_t timestampNs, int64_t frameDurationNs) {
    Metadata m = settings;
    m.set(ANDROID_SENSOR_TIMESTAMP, std::vector<int64_t>{timestampNs});
    m.set(ANDROID_SENSOR_FRAME_DURATION, std::vector<int64_t>{frameDurationNs});
    m.set(ANDROID_CONTROL_AE_STATE, std::vector<uint8_t>{ANDROID_CONTROL_AE_STATE_CONVERGED});
    m.set(ANDROID_CONTROL_AF_STATE, std::vector<uint8_t>{ANDROID_CONTROL_AF_STATE_INACTIVE});
    m.set(ANDROID_CONTROL_AWB_STATE, std::vector<uint8_t>{ANDROID_CONTROL_AWB_STATE_CONVERGED});
    m.set(ANDROID_FLASH_STATE, std::vector<uint8_t>{ANDROID_FLASH_STATE_UNAVAILABLE});
    m.set(ANDROID_LENS_STATE, std::vector<uint8_t>{ANDROID_LENS_STATE_STATIONARY});
    m.set(ANDROID_REQUEST_PIPELINE_DEPTH, std::vector<uint8_t>{kPipelineDepth});
    m.set(ANDROID_STATISTICS_SCENE_FLICKER, std::vector<uint8_t>{ANDROID_STATISTICS_SCENE_FLICKER_NONE});
    return m;
}

}  // namespace dsa::camera
