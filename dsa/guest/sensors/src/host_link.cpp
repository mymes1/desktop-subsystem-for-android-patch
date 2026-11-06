// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "host_link.h"

#include <sys/socket.h>
// After sys/socket.h, which defines what it uses.
#include <linux/vm_sockets.h>
#include <sys/time.h>
#include <unistd.h>

#include <cerrno>
#include <string>
#include <utility>

#include "log.h"

namespace dsa::sensors {

namespace {

// The largest frame the host sends: a batch of readings is a few kilobytes.
constexpr uint32_t kMaxHostFrame = 1024 * 1024;

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

void setReceiveTimeout(int fd, int seconds) {
    timeval timeout{seconds, 0};
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
}

}  // namespace

HostLink::HostLink(ReadingsHandler onReadings, ConnectedHandler onConnected)
    : mOnReadings(std::move(onReadings)), mOnConnected(std::move(onConnected)) {}

HostLink::~HostLink() {
    {
        std::lock_guard<std::mutex> lock(mMutex);
        mStopping = true;
        if (mFd >= 0) {
            shutdown(mFd, SHUT_RDWR);
        }
    }
    mChanged.notify_all();
    if (mThread.joinable()) {
        mThread.join();
    }
}

void HostLink::start() { mThread = std::thread([this] { run(); }); }

std::optional<std::vector<HostSensor>> HostLink::waitForList(std::chrono::milliseconds timeout) {
    std::unique_lock<std::mutex> lock(mMutex);
    mChanged.wait_for(lock, timeout, [this] { return mFirstList.has_value() || mStopping; });
    return mFirstList;
}

bool HostLink::send(const std::vector<uint8_t>& frame) {
    // Held while writing, so the reader cannot close the socket under the write.
    std::lock_guard<std::mutex> sending(mSendLock);
    int fd;
    {
        std::lock_guard<std::mutex> lock(mMutex);
        fd = mFd;
    }
    if (fd < 0) {
        return false;
    }
    return writeAll(fd, withLength(frame));
}

// Connects to the sensor port, does the handshake and reads the sensor list. Returns the
// socket, or -1 with the reason logged once in a row.
int HostLink::connectAndGreet(std::vector<HostSensor>* list) {
    const int fd = socket(AF_VSOCK, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        SENSORS_LOGE("cannot make a vsock socket (errno %d)", errno);
        return -1;
    }
    sockaddr_vm address{};
    address.svm_family = AF_VSOCK;
    address.svm_cid = VMADDR_CID_HOST;
    address.svm_port = kSensorPort;
    if (connect(fd, reinterpret_cast<const sockaddr*>(&address), sizeof(address)) != 0) {
        ::close(fd);
        return -1;
    }
    setReceiveTimeout(fd, 5);
    // A host that stops reading cannot hold up the framework's calls for long.
    timeval sendTimeout{5, 0};
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &sendTimeout, sizeof(sendTimeout));
    std::vector<uint8_t> frame;
    if (!writeAll(fd, withLength(encodeHello("dsa-sensors"))) || !readFrame(fd, &frame)) {
        SENSORS_LOGW("the host did not answer on the sensor port");
        ::close(fd);
        return -1;
    }
    const HostMessage hello = decodeHostFrame(frame.data(), frame.size());
    if (const auto* rejected = std::get_if<Rejected>(&hello)) {
        SENSORS_LOGE("the host refused the sensor port: %s", rejected->detail.c_str());
        ::close(fd);
        return -1;
    }
    if (!std::holds_alternative<Accepted>(hello) || !readFrame(fd, &frame)) {
        SENSORS_LOGW("the host answered the sensor port's hello with something else");
        ::close(fd);
        return -1;
    }
    HostMessage listed = decodeHostFrame(frame.data(), frame.size());
    auto* got = std::get_if<SensorListMessage>(&listed);
    if (got == nullptr) {
        SENSORS_LOGW("the host did not say which sensors it has");
        ::close(fd);
        return -1;
    }
    *list = std::move(got->sensors);
    // Readings come only while a sensor is on; the reader waits as long as it takes.
    setReceiveTimeout(fd, 0);
    return fd;
}

void HostLink::run() {
    bool warned = false;
    while (true) {
        {
            std::lock_guard<std::mutex> lock(mMutex);
            if (mStopping) return;
        }
        std::vector<HostSensor> list;
        const int fd = connectAndGreet(&list);
        if (fd < 0) {
            if (!warned) {
                SENSORS_LOGI("waiting for the host's sensor port (the DSA window manager)");
                warned = true;
            }
            std::unique_lock<std::mutex> lock(mMutex);
            mChanged.wait_for(lock, std::chrono::seconds(2), [this] { return mStopping; });
            continue;
        }
        warned = false;
        SENSORS_LOGI("connected to the host's sensors: it has %zu", list.size());
        {
            std::lock_guard<std::mutex> lock(mMutex);
            if (mStopping) {
                ::close(fd);
                return;
            }
            mFd = fd;
            if (!mFirstList) {
                mFirstList = std::move(list);
            }
        }
        mChanged.notify_all();
        mOnConnected();

        std::vector<uint8_t> frame;
        while (readFrame(fd, &frame)) {
            const HostMessage message = decodeHostFrame(frame.data(), frame.size());
            if (const auto* readings = std::get_if<ReadingsMessage>(&message)) {
                mOnReadings(readings->readings);
            }
        }

        {
            // A send under way finishes before the socket closes.
            std::lock_guard<std::mutex> sending(mSendLock);
            {
                std::lock_guard<std::mutex> lock(mMutex);
                mFd = -1;
            }
            ::close(fd);
        }
        SENSORS_LOGW("the host's sensor port closed; connecting again");
    }
}

}  // namespace dsa::sensors
