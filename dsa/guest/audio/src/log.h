// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#pragma once

#ifdef __ANDROID__
#include <android/log.h>

#define DSA_AUDIO_TAG "dsa-audio"

#define AUDIO_LOGE(...) __android_log_print(ANDROID_LOG_ERROR, DSA_AUDIO_TAG, __VA_ARGS__)
#define AUDIO_LOGW(...) __android_log_print(ANDROID_LOG_WARN, DSA_AUDIO_TAG, __VA_ARGS__)
#define AUDIO_LOGI(...) __android_log_print(ANDROID_LOG_INFO, DSA_AUDIO_TAG, __VA_ARGS__)
#else
// The tests build on the build machine, where the messages go to stderr.
#include <cstdio>

#define AUDIO_LOGE(...) (std::fprintf(stderr, __VA_ARGS__), std::fputc('\n', stderr))
#define AUDIO_LOGW(...) AUDIO_LOGE(__VA_ARGS__)
#define AUDIO_LOGI(...) AUDIO_LOGE(__VA_ARGS__)
#endif
