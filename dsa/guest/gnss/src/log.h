// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// Logging for the GNSS HAL, under the tag dsa-gnss.

#pragma once

#include <android/log.h>

#ifndef LOG_TAG
#define LOG_TAG "dsa-gnss"
#endif

#define GNSS_LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)
#define GNSS_LOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define GNSS_LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
