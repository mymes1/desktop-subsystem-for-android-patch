// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The camera HAL's connections to the host's camera port (vsock 5572). One short connection
// lists the host's cameras; a session holds one for its camera while it streams, with a
// thread of the link's own that reads the host's pictures and answers.

#pragma once

#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <vector>

#include "camera_messages.h"

namespace dsa::camera {

// The vsock port of the host's cameras.
constexpr uint32_t kCameraPort = 5572;

// The cameras the host lends the guest now. Nothing when the host is not there (its window
// manager is not running) or refuses; `error` says why.
std::optional<std::vector<HostCamera>> listHostCameras(std::string* error);

class HostCameraLink {
   public:
    ~HostCameraLink();

    // Connects to the camera port. On failure returns nothing and says why in `error`.
    static std::unique_ptr<HostCameraLink> connect(std::string* error);

    // Starts the camera `id`, or changes its size or rate, and waits for the host's answer.
    // Pictures from before the change are dropped.
    bool start(const std::string& id, uint32_t width, uint32_t height, uint32_t fps, Opened* opened,
               std::string* error);

    // Stops the camera; the host lets it go.
    void stop();

    // Waits up to `timeout` for a picture newer than the one numbered `after` (or any, when
    // `after` is empty). False, with the reason in `error`, when none comes or the camera
    // failed.
    bool nextPicture(std::optional<uint64_t> after, std::chrono::milliseconds timeout,
                     std::shared_ptr<const Picture>* picture, std::string* error);

    // A JPEG still of `width` by `height` from the newest picture.
    bool still(uint32_t width, uint32_t height, uint32_t quality, std::string* jpeg, std::string* error);

    // Closes the connection and waits for the reader to end.
    void close();

   private:
    explicit HostCameraLink(int fd);
    bool send(const std::vector<uint8_t>& frame);
    void read();

    int mFd;
    std::mutex mSendLock;
    std::mutex mLock;
    std::condition_variable mChanged;
    bool mClosed = false;
    std::string mFailure;
    std::optional<Opened> mOpened;
    std::shared_ptr<const Picture> mLatest;
    uint32_t mNextStill = 1;
    std::map<uint32_t, StillTaken> mStills;
    std::thread mReader;
};

}  // namespace dsa::camera
