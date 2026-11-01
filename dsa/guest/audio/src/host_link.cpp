// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "host_link.h"

#include <fcntl.h>
#include <linux/vm_sockets.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

#include <cerrno>
#include <chrono>
#include <cstring>
#include <variant>
#include <vector>

#include "audio_messages.h"
#include "log.h"

namespace dsa::audio {

namespace {

// 200 ms of sound with its framing: past this the host is not reading, and sound is dropped.
constexpr size_t kOutboundLimit = 48000 * 4 / 5 + 4096;
// The largest frame the host may send the HAL.
constexpr uint32_t kMaxHostFrame = 64 * 1024;
constexpr auto kRetryDelay = std::chrono::seconds(1);
constexpr auto kRefusedDelay = std::chrono::seconds(10);

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

// One frame from the host, within the socket's receive timeout.
bool readFrame(int fd, std::vector<uint8_t>* frame) {
    uint8_t header[4];
    if (!readAll(fd, header, sizeof(header))) return false;
    const uint32_t size = static_cast<uint32_t>(header[0]) | (static_cast<uint32_t>(header[1]) << 8) |
                          (static_cast<uint32_t>(header[2]) << 16) |
                          (static_cast<uint32_t>(header[3]) << 24);
    if (size > kMaxHostFrame) return false;
    frame->resize(size);
    return readAll(fd, frame->data(), size);
}

}  // namespace

int connectToHost() {
    const int fd = socket(AF_VSOCK, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        return -1;
    }
    sockaddr_vm address{};
    address.svm_family = AF_VSOCK;
    address.svm_cid = VMADDR_CID_HOST;
    address.svm_port = kAudioPort;
    if (connect(fd, reinterpret_cast<const sockaddr*>(&address), sizeof(address)) != 0) {
        close(fd);
        return -1;
    }
    return fd;
}

HostLink::HostLink(std::string name, uint32_t framesPerBurst, std::function<int()> connect)
    : mName(std::move(name)),
      mFramesPerBurst(framesPerBurst),
      mConnect(std::move(connect)),
      mOutbound(kOutboundLimit) {
    mThread = std::thread([this] { run(); });
}

HostLink::~HostLink() {
    {
        std::lock_guard<std::mutex> lock(mLock);
        mStopping = true;
        if (mSocket.ok()) {
            shutdown(mSocket.get(), SHUT_RDWR);
        }
    }
    mWake.notify_all();
    if (mThread.joinable()) {
        mThread.join();
    }
}

bool HostLink::open(int fd) {
    timeval timeout{2, 0};
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
    std::vector<uint8_t> frame;
    if (!writeAll(fd, withLength(encodeHello("dsa-audio"))) || !readFrame(fd, &frame)) {
        return false;
    }
    const HostMessage hello = decodeHostFrame(frame.data(), frame.size());
    if (const auto* rejected = std::get_if<Rejected>(&hello)) {
        AUDIO_LOGE("the host refused the %s stream: %s", mName.c_str(), rejected->detail.c_str());
        return false;
    }
    if (!std::holds_alternative<Accepted>(hello)) {
        AUDIO_LOGE("the host answered the %s stream's hello with something else", mName.c_str());
        return false;
    }
    if (!writeAll(fd, withLength(encodeOpen(mName, mFramesPerBurst))) || !readFrame(fd, &frame)) {
        return false;
    }
    const HostMessage ready = decodeHostFrame(frame.data(), frame.size());
    const auto* r = std::get_if<Ready>(&ready);
    if (r == nullptr) {
        AUDIO_LOGE("the host did not open the %s stream", mName.c_str());
        return false;
    }
    mHostDelay.store(r->bufferedFrames + r->deviceFrames);
    // From here the stream's thread sends, and must never wait on the socket.
    fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK);
    return true;
}

void HostLink::run() {
    bool loggedRetry = false;
    for (;;) {
        {
            std::unique_lock<std::mutex> lock(mLock);
            if (mStopping) return;
        }
        UniqueFd fd(mConnect());
        bool refused = false;
        if (fd.ok() && !open(fd.get())) {
            refused = true;
            fd.reset();
        }
        if (!fd.ok()) {
            if (!loggedRetry) {
                AUDIO_LOGW("the host's audio port is not answering; the %s stream is silent until it does",
                           mName.c_str());
                loggedRetry = true;
            }
            std::unique_lock<std::mutex> lock(mLock);
            mWake.wait_for(lock, refused ? kRefusedDelay : kRetryDelay, [this] { return mStopping; });
            continue;
        }
        loggedRetry = false;
        AUDIO_LOGI("the %s stream reaches the host", mName.c_str());
        const int socket = fd.get();
        {
            std::lock_guard<std::mutex> lock(mLock);
            mSocket = std::move(fd);
            mOutbound.clear();
            if (mHasState) {
                sendLocked(withLength(encodeControl(mState)), false, 0);
            }
        }
        // Watch for the host going away. It sends nothing more after AudioOutputReady.
        for (;;) {
            pollfd p{socket, POLLIN, 0};
            const int ready = poll(&p, 1, 200);
            std::lock_guard<std::mutex> lock(mLock);
            if (mStopping || !mSocket.ok() || mSocket.get() != socket) {
                break;
            }
            if (ready > 0) {
                uint8_t scratch[256];
                const ssize_t n = recv(socket, scratch, sizeof(scratch), MSG_DONTWAIT);
                if (n == 0 || (n < 0 && errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR)) {
                    lost();
                    break;
                }
            }
        }
        AUDIO_LOGW("the %s stream lost the host; connecting again", mName.c_str());
    }
}

void HostLink::lost() {
    mSocket.reset();
    mOutbound.clear();
}

void HostLink::sendLocked(const std::vector<uint8_t>& frame, bool isPcm, size_t frames) {
    if (!mSocket.ok()) {
        if (isPcm) mDropped += frames;
        return;
    }
    if (!mOutbound.push(frame) && isPcm) {
        mDropped += frames;
    }
    const int fd = mSocket.get();
    const bool ok = mOutbound.flush([fd](const uint8_t* data, size_t size) -> long {
        for (;;) {
            const ssize_t n = send(fd, data, size, MSG_DONTWAIT | MSG_NOSIGNAL);
            if (n >= 0) return static_cast<long>(n);
            if (errno == EINTR) continue;
            if (errno == EAGAIN || errno == EWOULDBLOCK) return 0;
            return -1;
        }
    });
    if (!ok) {
        lost();
        mWake.notify_all();
    }
}

void HostLink::pcm(uint64_t position, const uint8_t* data, size_t size) {
    std::lock_guard<std::mutex> lock(mLock);
    sendLocked(withLength(encodePcm(position, data, size)), true, size / 4);
}

void HostLink::command(HostCommand command) {
    std::lock_guard<std::mutex> lock(mLock);
    if (command != HostCommand::Flush) {
        mHasState = true;
        mState = command;
    }
    sendLocked(withLength(encodeControl(command)), false, 0);
}

}  // namespace dsa::audio
