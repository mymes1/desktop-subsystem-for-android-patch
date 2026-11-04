// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#pragma once

#include <android/log.h>

#define DSA_CAMERA_TAG "dsa-camera"

#define CAMERA_LOGE(...) __android_log_print(ANDROID_LOG_ERROR, DSA_CAMERA_TAG, __VA_ARGS__)
#define CAMERA_LOGW(...) __android_log_print(ANDROID_LOG_WARN, DSA_CAMERA_TAG, __VA_ARGS__)
#define CAMERA_LOGI(...) __android_log_print(ANDROID_LOG_INFO, DSA_CAMERA_TAG, __VA_ARGS__)
