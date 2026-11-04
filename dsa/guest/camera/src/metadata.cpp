// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "metadata.h"

#include <cstdlib>
#include <cstring>

namespace dsa::camera {

namespace {

constexpr size_t kEntries = 64;
constexpr size_t kData = 4096;

}  // namespace

Metadata::Metadata() : mBuffer(allocate_camera_metadata(kEntries, kData)) {}

Metadata::~Metadata() {
    if (mBuffer != nullptr) {
        free_camera_metadata(mBuffer);
    }
}

Metadata::Metadata(const Metadata& other) : mBuffer(clone_camera_metadata(other.mBuffer)) {}

Metadata& Metadata::operator=(const Metadata& other) {
    if (this != &other) {
        camera_metadata_t* copy = clone_camera_metadata(other.mBuffer);
        if (mBuffer != nullptr) {
            free_camera_metadata(mBuffer);
        }
        mBuffer = copy;
    }
    return *this;
}

Metadata Metadata::fromBytes(const std::vector<uint8_t>& bytes, bool* valid) {
    Metadata out;
    *valid = false;
    if (bytes.empty()) {
        return out;
    }
    // The bytes may sit anywhere; the structure wants its own alignment.
    void* aligned = std::aligned_alloc(16, (bytes.size() + 15) / 16 * 16);
    if (aligned == nullptr) {
        return out;
    }
    std::memcpy(aligned, bytes.data(), bytes.size());
    const auto* source = static_cast<const camera_metadata_t*>(aligned);
    const size_t size = bytes.size();
    if (validate_camera_metadata_structure(source, &size) == 0) {
        camera_metadata_t* copy = clone_camera_metadata(source);
        if (copy != nullptr) {
            free_camera_metadata(out.mBuffer);
            out.mBuffer = copy;
            *valid = true;
        }
    }
    std::free(aligned);
    return out;
}

bool Metadata::setRaw(uint32_t tag, int type, const void* data, size_t count) {
    if (mBuffer == nullptr || get_camera_metadata_tag_type(tag) != type) {
        return false;
    }
    camera_metadata_entry_t entry;
    if (find_camera_metadata_entry(mBuffer, tag, &entry) == 0) {
        if (update_camera_metadata_entry(mBuffer, entry.index, data, count, nullptr) == 0) {
            return true;
        }
        // No room for the larger value: it goes again at the end of a larger buffer.
        delete_camera_metadata_entry(mBuffer, entry.index);
    }
    if (add_camera_metadata_entry(mBuffer, tag, data, count) == 0) {
        return true;
    }
    const size_t entries = get_camera_metadata_entry_capacity(mBuffer) * 2 + 1;
    const size_t bytes = get_camera_metadata_data_capacity(mBuffer) * 2 +
                         static_cast<size_t>(calculate_camera_metadata_entry_data_size(type, count));
    camera_metadata_t* larger = allocate_camera_metadata(entries, bytes);
    if (larger == nullptr || append_camera_metadata(larger, mBuffer) != 0) {
        if (larger != nullptr) {
            free_camera_metadata(larger);
        }
        return false;
    }
    free_camera_metadata(mBuffer);
    mBuffer = larger;
    return add_camera_metadata_entry(mBuffer, tag, data, count) == 0;
}

bool Metadata::set(uint32_t tag, const std::vector<uint8_t>& values) {
    return setRaw(tag, TYPE_BYTE, values.data(), values.size());
}

bool Metadata::set(uint32_t tag, const std::vector<int32_t>& values) {
    return setRaw(tag, TYPE_INT32, values.data(), values.size());
}

bool Metadata::set(uint32_t tag, const std::vector<int64_t>& values) {
    return setRaw(tag, TYPE_INT64, values.data(), values.size());
}

bool Metadata::set(uint32_t tag, const std::vector<float>& values) {
    return setRaw(tag, TYPE_FLOAT, values.data(), values.size());
}

bool Metadata::set(uint32_t tag, const std::vector<camera_metadata_rational_t>& values) {
    return setRaw(tag, TYPE_RATIONAL, values.data(), values.size());
}

bool Metadata::get(uint32_t tag, uint8_t* value) const {
    camera_metadata_ro_entry_t entry;
    if (mBuffer == nullptr || find_camera_metadata_ro_entry(mBuffer, tag, &entry) != 0 || entry.count == 0 ||
        entry.type != TYPE_BYTE) {
        return false;
    }
    *value = entry.data.u8[0];
    return true;
}

bool Metadata::get(uint32_t tag, int32_t* value) const {
    camera_metadata_ro_entry_t entry;
    if (mBuffer == nullptr || find_camera_metadata_ro_entry(mBuffer, tag, &entry) != 0 || entry.count == 0 ||
        entry.type != TYPE_INT32) {
        return false;
    }
    *value = entry.data.i32[0];
    return true;
}

bool Metadata::getAll(uint32_t tag, std::vector<int32_t>* values) const {
    camera_metadata_ro_entry_t entry;
    if (mBuffer == nullptr || find_camera_metadata_ro_entry(mBuffer, tag, &entry) != 0 ||
        entry.type != TYPE_INT32) {
        return false;
    }
    values->assign(entry.data.i32, entry.data.i32 + entry.count);
    return true;
}

std::vector<int32_t> Metadata::tags() const {
    std::vector<int32_t> out;
    if (mBuffer == nullptr) {
        return out;
    }
    const size_t count = get_camera_metadata_entry_count(mBuffer);
    for (size_t i = 0; i < count; ++i) {
        camera_metadata_ro_entry_t entry;
        if (get_camera_metadata_ro_entry(mBuffer, i, &entry) == 0) {
            out.push_back(static_cast<int32_t>(entry.tag));
        }
    }
    return out;
}

bool Metadata::empty() const { return mBuffer == nullptr || get_camera_metadata_entry_count(mBuffer) == 0; }

std::vector<uint8_t> Metadata::bytes() const {
    if (mBuffer == nullptr) {
        return {};
    }
    const auto* data = reinterpret_cast<const uint8_t*>(mBuffer);
    return std::vector<uint8_t>(data, data + get_camera_metadata_size(mBuffer));
}

}  // namespace dsa::camera
