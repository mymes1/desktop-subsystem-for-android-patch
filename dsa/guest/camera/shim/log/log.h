// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// Stands in for the platform's <log/log.h> (liblog's full header, which the NDK does not
// ship) for the camera metadata sources: the few macros they use, on the NDK's log.

#pragma once

#include <android/log.h>

#ifndef LOG_TAG
#define LOG_TAG "camera_metadata"
#endif

#define ALOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)
#define ALOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)
#define ALOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define ALOGV(...) ((void)0)

// The platform records these in the security event log; here they go to the error log.
#define android_errorWriteLog(tag, subTag) ALOGE("security event %x %s", (unsigned)(tag), (subTag))
