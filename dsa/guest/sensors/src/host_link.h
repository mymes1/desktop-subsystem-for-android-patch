// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The HAL's connection to the host's sensor port (vsock 5580), which the host's window
// manager serves. A thread of the link's own connects, reads the host's sensor list, and then
// hands each batch of readings on. When the host goes (its window manager restarts), the link
// connects again and says so, so the sensors the framework has on are asked for again.

#pragma once

#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <functional>
#include <mutex>
#include <optional>
#include <thread>
#include <vector>

#include "sensor_messages.h"

namespace dsa::sensors {

// The vsock port of the host's sensors.
constexpr uint32_t kSensorPort = 5580;

class HostLink {
   public:
    using ReadingsHandler = std::function<void(const std::vector<Reading>&)>;
    using ConnectedHandler = std::function<void()>;

    HostLink(ReadingsHandler onReadings, ConnectedHandler onConnected);
    ~HostLink();

    void start();

    // Waits at most `timeout` for the host's first sensor list. Nothing when it did not come.
    std::optional<std::vector<HostSensor>> waitForList(std::chrono::milliseconds timeout);

    // Sends one encoded Envelope. False when the host is not connected; what it asked for is
    // asked again when the host connects.
    bool send(const std::vector<uint8_t>& frame);

   private:
    void run();
    int connectAndGreet(std::vector<HostSensor>* list);

    ReadingsHandler mOnReadings;
    ConnectedHandler mOnConnected;

    std::mutex mMutex;
    std::condition_variable mChanged;
    int mFd = -1;
    bool mStopping = false;
    std::optional<std::vector<HostSensor>> mFirstList;
    std::mutex mSendLock;
    std::thread mThread;
};

}  // namespace dsa::sensors
