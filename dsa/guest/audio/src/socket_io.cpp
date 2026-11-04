// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "socket_io.h"

#include <sys/socket.h>
#include <sys/types.h>

#include <cerrno>

namespace dsa::audio {

bool writeAll(int fd, const std::vector<uint8_t>& bytes) {
    size_t done = 0;
    while (done < bytes.size()) {
        const ssize_t n = send(fd, bytes.data() + done, bytes.size() - done, MSG_NOSIGNAL);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return false;
        done += static_cast<size_t>(n);
    }
    return true;
}

bool readAll(int fd, uint8_t* out, size_t size) {
    size_t done = 0;
    while (done < size) {
        const ssize_t n = recv(fd, out + done, size - done, 0);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return false;
        done += static_cast<size_t>(n);
    }
    return true;
}

bool readFrame(int fd, std::vector<uint8_t>* frame) {
    uint8_t header[4];
    if (!readAll(fd, header, sizeof(header))) return false;
    const uint32_t size = static_cast<uint32_t>(header[0]) | (static_cast<uint32_t>(header[1]) << 8) |
                          (static_cast<uint32_t>(header[2]) << 16) |
                          (static_cast<uint32_t>(header[3]) << 24);
    if (size > kMaxHostFrame) return false;
    frame->resize(size);
    return size == 0 || readAll(fd, frame->data(), size);
}

}  // namespace dsa::audio
