// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// What Android is told about each host camera (its static characteristics), the settings
// each request template starts from, and the result each capture reports.
//
// A host camera is a fixed focus camera whose exposure and white balance it runs itself, as
// a USB camera does. It is described at Android's EXTERNAL hardware level: preview, video and
// JPEG stills at the sizes and rates the host camera has, with nothing manual.

#pragma once

#include <cstdint>

#include "camera_messages.h"
#include "metadata.h"

namespace dsa::camera {

// The static characteristics of `camera`.
Metadata characteristics(const HostCamera& camera);

// The settings Android's request template `templateId` starts from (PREVIEW 1, STILL_CAPTURE
// 2, VIDEO_RECORD 3, VIDEO_SNAPSHOT 4). False for a template the camera does not offer
// (ZERO_SHUTTER_LAG, MANUAL).
bool defaultSettings(const HostCamera& camera, int32_t templateId, Metadata* settings);

// The result of a capture made with `settings`: the settings, and what a camera that runs
// itself reports, taken at `timestampNs`.
Metadata captureResult(const Metadata& settings, int64_t timestampNs, int64_t frameDurationNs);

// The largest JPEG a still of `camera` makes, with the trailer that says its length: what
// Android sizes still buffers by.
int32_t jpegMaxSize(const HostCamera& camera);

}  // namespace dsa::camera
