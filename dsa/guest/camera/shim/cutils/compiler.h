// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// Stands in for the platform's <cutils/compiler.h> for the camera metadata sources, which
// use only its export marker.

#pragma once

#ifndef ANDROID_API
#define ANDROID_API __attribute__((visibility("default")))
#endif
