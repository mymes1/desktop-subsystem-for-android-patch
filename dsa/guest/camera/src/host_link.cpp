// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "host_link.h"

#include <sys/socket.h>
// After sys/socket.h, which defines what it uses.
#include <linux/vm_sockets.h>
#include <sys/time.h>
#include <unistd.h>

#include <cerrno>

#include "log.h"

namespace dsa::camera {

namespace {

// The largest frame the host may send: a 4096 by 2304 picture with its framing, and room.
constexpr uint32_t kMaxHostFrame = 16 * 1024 * 1024;
constexpr std::chrono::seconds kAnswerWait{5};

bool writeAll(int fd, const std::vector<uint8_t>& bytes) {
    size_t done = 0;
    while (done < bytes.size()) {
        const ssize_t n = ::send(fd, bytes.data() + done, bytes.size() - done, MSG_NOSIGNAL);
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
                          (static_cast<uint32_t>(header[2]) << 16) | (static_cast<uint32_t>(header[3]) << 24);
    if (size > kMaxHostFrame) return false;
    frame->resize(size);
    return readAll(fd, frame->data(), size);
}

void setTimeout(int fd, int seconds) {
    timeval timeout{seconds, 0};
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));
}

// Connects to the camera port and does the handshake; reads the host's camera list, which
// comes next. Returns the socket, or -1 with the reason in `error`.
int connectAndGreet(std::string* error, CameraList* list) {
    const int fd = socket(AF_VSOCK, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        *error = "cannot make a vsock socket";
        return -1;
    }
    sockaddr_vm address{};
    address.svm_family = AF_VSOCK;
    address.svm_cid = VMADDR_CID_HOST;
    address.svm_port = kCameraPort;
    if (connect(fd, reinterpret_cast<const sockaddr*>(&address), sizeof(address)) != 0) {
        *error = "the host's camera port is not open (is the DSA window manager running?)";
        ::close(fd);
        return -1;
    }
    setTimeout(fd, 5);
    std::vector<uint8_t> frame;
    if (!writeAll(fd, withLength(encodeHello("dsa-camera"))) || !readFrame(fd, &frame)) {
        *error = "the host did not answer on the camera port";
        ::close(fd);
        return -1;
    }
    const HostMessage hello = decodeHostFrame(frame.data(), frame.size());
    if (const auto* rejected = std::get_if<Rejected>(&hello)) {
        *error = "the host refused the camera port: " + rejected->detail;
        ::close(fd);
        return -1;
    }
    if (!std::holds_alternative<Accepted>(hello) || !readFrame(fd, &frame)) {
        *error = "the host answered the camera port's hello with something else";
        ::close(fd);
        return -1;
    }
    HostMessage listed = decodeHostFrame(frame.data(), frame.size());
    auto* got = std::get_if<CameraList>(&listed);
    if (got == nullptr) {
        *error = "the host did not list its cameras";
        ::close(fd);
        return -1;
    }
    *list = std::move(*got);
    return fd;
}

}  // namespace

std::optional<std::vector<HostCamera>> listHostCameras(std::string* error) {
    CameraList list;
    const int fd = connectAndGreet(error, &list);
    if (fd < 0) {
        return std::nullopt;
    }
    ::close(fd);
    return std::move(list.cameras);
}

std::unique_ptr<HostCameraLink> HostCameraLink::connect(std::string* error) {
    CameraList list;
    const int fd = connectAndGreet(error, &list);
    if (fd < 0) {
        return nullptr;
    }
    // Pictures come as long as the camera runs; the reader waits as long as it takes.
    timeval none{0, 0};
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &none, sizeof(none));
    std::unique_ptr<HostCameraLink> link(new HostCameraLink(fd));
    link->mReader = std::thread([raw = link.get()] { raw->read(); });
    return link;
}

HostCameraLink::HostCameraLink(int fd) : mFd(fd) {}

HostCameraLink::~HostCameraLink() { close(); }

bool HostCameraLink::send(const std::vector<uint8_t>& frame) {
    std::lock_guard<std::mutex> lock(mSendLock);
    if (mFd < 0) {
        return false;
    }
    return writeAll(mFd, withLength(frame));
}

void HostCameraLink::read() {
    std::vector<uint8_t> frame;
    while (readFrame(mFd, &frame)) {
        HostMessage message = decodeHostFrame(frame.data(), frame.size());
        std::lock_guard<std::mutex> lock(mLock);
        if (mClosed) {
            return;
        }
        if (auto* opened = std::get_if<Opened>(&message)) {
            mOpened = *opened;
            mLatest.reset();
            mFailure.clear();
        } else if (auto* picture = std::get_if<Picture>(&message)) {
            mLatest = std::make_shared<const Picture>(std::move(*picture));
        } else if (auto* still = std::get_if<StillTaken>(&message)) {
            mStills[still->requestId] = std::move(*still);
        } else if (auto* failed = std::get_if<CameraFailed>(&message)) {
            CAMERA_LOGW("the host's camera failed: %s", failed->error.c_str());
            mFailure = failed->error;
        }
        mChanged.notify_all();
    }
    std::lock_guard<std::mutex> lock(mLock);
    if (!mClosed) {
        CAMERA_LOGW("the host's camera went away");
        mFailure = "the host's camera went away";
        mClosed = true;
    }
    mChanged.notify_all();
}

bool HostCameraLink::start(const std::string& id, uint32_t width, uint32_t height, uint32_t fps, Opened* opened,
                           std::string* error) {
    {
        std::lock_guard<std::mutex> lock(mLock);
        mOpened.reset();
    }
    if (!send(encodeOpen(id, width, height, fps))) {
        *error = "the host's camera port closed";
        return false;
    }
    std::unique_lock<std::mutex> lock(mLock);
    if (!mChanged.wait_for(lock, kAnswerWait, [&] { return mOpened.has_value() || mClosed; }) || !mOpened) {
        *error = mClosed ? mFailure : "the host did not answer the request for its camera";
        return false;
    }
    *opened = *mOpened;
    if (!opened->ok) {
        *error = opened->error;
        return false;
    }
    return true;
}

void HostCameraLink::stop() {
    send(encodeStop());
    std::lock_guard<std::mutex> lock(mLock);
    mOpened.reset();
    mLatest.reset();
}

bool HostCameraLink::nextPicture(std::optional<uint64_t> after, std::chrono::milliseconds timeout,
                                 std::shared_ptr<const Picture>* picture, std::string* error) {
    std::unique_lock<std::mutex> lock(mLock);
    const auto ready = [&] {
        return mClosed || !mFailure.empty() || (mLatest && (!after || mLatest->sequence > *after));
    };
    if (!mChanged.wait_for(lock, timeout, ready)) {
        *error = "the host's camera sent no picture in time";
        return false;
    }
    if (mClosed || !mFailure.empty()) {
        *error = mFailure.empty() ? "the host's camera went away" : mFailure;
        return false;
    }
    *picture = mLatest;
    return true;
}

bool HostCameraLink::still(uint32_t width, uint32_t height, uint32_t quality, std::string* jpeg, std::string* error) {
    uint32_t request = 0;
    {
        std::lock_guard<std::mutex> lock(mLock);
        request = mNextStill++;
    }
    if (!send(encodeStill(request, width, height, quality))) {
        *error = "the host's camera port closed";
        return false;
    }
    std::unique_lock<std::mutex> lock(mLock);
    const bool answered =
        mChanged.wait_for(lock, kAnswerWait, [&] { return mClosed || mStills.count(request) != 0; });
    auto taken = mStills.find(request);
    if (!answered || taken == mStills.end()) {
        *error = mClosed ? mFailure : "the host did not make the still in time";
        return false;
    }
    StillTaken result = std::move(taken->second);
    mStills.erase(taken);
    if (result.jpeg.empty()) {
        *error = result.error.empty() ? "the host made no still" : result.error;
        return false;
    }
    *jpeg = std::move(result.jpeg);
    return true;
}

void HostCameraLink::close() {
    {
        std::lock_guard<std::mutex> lock(mLock);
        if (mClosed && !mReader.joinable() && mFd < 0) {
            return;
        }
        mClosed = true;
        mChanged.notify_all();
    }
    {
        std::lock_guard<std::mutex> lock(mSendLock);
        if (mFd >= 0) {
            shutdown(mFd, SHUT_RDWR);
        }
    }
    if (mReader.joinable()) {
        if (mReader.get_id() == std::this_thread::get_id()) {
            mReader.detach();
        } else {
            mReader.join();
        }
    }
    std::lock_guard<std::mutex> lock(mSendLock);
    if (mFd >= 0) {
        ::close(mFd);
        mFd = -1;
    }
}

}  // namespace dsa::camera
