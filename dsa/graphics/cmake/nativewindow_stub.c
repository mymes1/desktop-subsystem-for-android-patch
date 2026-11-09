// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// Link-time stand-in for the platform's libnativewindow.so, with the AHardwareBuffer
// functions the guest Vulkan driver calls. The NDK's copy lacks the vendor-only
// AHardwareBuffer_getNativeHandle and AHardwareBuffer_createFromHandle, with which the driver
// locks a buffer it has only the handle of. Only the symbols matter: on the device the driver
// loads the platform's own library.

#include <stdint.h>

typedef struct AHardwareBuffer AHardwareBuffer;
typedef struct AHardwareBuffer_Desc AHardwareBuffer_Desc;
typedef struct ARect ARect;
typedef struct native_handle native_handle_t;

int AHardwareBuffer_allocate(const AHardwareBuffer_Desc* desc, AHardwareBuffer** out) { return -1; }
void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {}
void AHardwareBuffer_release(AHardwareBuffer* buffer) {}
void AHardwareBuffer_describe(const AHardwareBuffer* buffer, AHardwareBuffer_Desc* desc) {}
int AHardwareBuffer_lock(AHardwareBuffer* buffer, uint64_t usage, int32_t fence,
                         const ARect* rect, void** address) { return -1; }
int AHardwareBuffer_unlock(AHardwareBuffer* buffer, int32_t* fence) { return -1; }
int AHardwareBuffer_getId(const AHardwareBuffer* buffer, uint64_t* id) { return -1; }
const native_handle_t* AHardwareBuffer_getNativeHandle(const AHardwareBuffer* buffer) { return 0; }
int AHardwareBuffer_createFromHandle(const AHardwareBuffer_Desc* desc,
                                     const native_handle_t* handle, int32_t method,
                                     AHardwareBuffer** out) { return -1; }
