// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// Link-time stand-in for the libnativewindow.so call the camera HAL makes that the NDK's copy
// of the library leaves out: importing a graphics buffer handle, which the platform offers to
// vendor programs. Only the symbol matters: on the device the HAL loads the platform's own
// library. check-guest-driver.py checks it resolves there.

#include <stdint.h>

typedef struct AHardwareBuffer AHardwareBuffer;
typedef struct AHardwareBuffer_Desc AHardwareBuffer_Desc;

int AHardwareBuffer_createFromHandle(const AHardwareBuffer_Desc* desc, const void* handle, int32_t method,
                                     AHardwareBuffer** outBuffer) {
    return -1;
}
