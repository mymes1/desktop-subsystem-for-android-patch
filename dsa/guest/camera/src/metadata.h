// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// Camera metadata as the camera framework carries it: a camera_metadata_t buffer, built and
// read with the platform's own camera metadata code (system/media/camera), which build.sh
// fetches. The AIDL CameraMetadata parcelable holds the buffer's bytes as they are.

#pragma once

#include <system/camera_metadata.h>

#include <cstdint>
#include <vector>

namespace dsa::camera {

class Metadata {
   public:
    Metadata();
    ~Metadata();
    Metadata(const Metadata& other);
    Metadata& operator=(const Metadata& other);

    // The metadata whose bytes are `bytes`; empty when they are not valid camera metadata.
    static Metadata fromBytes(const std::vector<uint8_t>& bytes, bool* valid);

    // Sets `tag` to `values`, in place of what it held. False when the tag does not take
    // values of that type.
    bool set(uint32_t tag, const std::vector<uint8_t>& values);
    bool set(uint32_t tag, const std::vector<int32_t>& values);
    bool set(uint32_t tag, const std::vector<int64_t>& values);
    bool set(uint32_t tag, const std::vector<float>& values);
    bool set(uint32_t tag, const std::vector<camera_metadata_rational_t>& values);

    // The first value of `tag`, when it has one of that type.
    bool get(uint32_t tag, uint8_t* value) const;
    bool get(uint32_t tag, int32_t* value) const;
    // Every value of `tag`, when it is of that type.
    bool getAll(uint32_t tag, std::vector<int32_t>* values) const;

    // Every tag set, in the order they were added.
    std::vector<int32_t> tags() const;

    bool empty() const;
    std::vector<uint8_t> bytes() const;

   private:
    bool setRaw(uint32_t tag, int type, const void* data, size_t count);

    camera_metadata_t* mBuffer = nullptr;
};

}  // namespace dsa::camera
