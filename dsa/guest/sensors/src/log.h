// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// Logging for the sensors HAL (dsa-sensors).

#pragma once

#include <android/log.h>

#define SENSORS_LOGE(...) __android_log_print(ANDROID_LOG_ERROR, "dsa-sensors", __VA_ARGS__)
#define SENSORS_LOGW(...) __android_log_print(ANDROID_LOG_WARN, "dsa-sensors", __VA_ARGS__)
#define SENSORS_LOGI(...) __android_log_print(ANDROID_LOG_INFO, "dsa-sensors", __VA_ARGS__)
