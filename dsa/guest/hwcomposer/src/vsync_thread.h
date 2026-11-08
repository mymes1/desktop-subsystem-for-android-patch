// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The display's vsync: the guest display has no hardware behind it, so a clock stands in,
// ticking at the display's refresh rate while SurfaceFlinger wants vsync events.
//
// SurfaceFlinger paces every app's frames from these ticks, so a tick that comes late makes
// every app late. The thread therefore shares no lock with composition (a present can take
// as long as the host does), and its owner raises it to real-time priority in `onStart`:
// at normal priority, in the composer's low-capacity cpuset, background work such as dex2oat
// keeps it off the CPU for a second at a time.

#pragma once

#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <functional>
#include <mutex>
#include <thread>

namespace dsa::hwc {

// The first tick on the grid `previous + k * period` (k >= 1) that is still ahead of `now`.
// Ticks missed while the thread was off the CPU are skipped, never replayed in a burst.
int64_t nextVsyncAfter(int64_t previous, int64_t now, int32_t periodNanos);

class VsyncThread {
   public:
    // `onVsync` gets each tick's CLOCK_MONOTONIC time in nanoseconds, called with no lock
    // held. `onStart`, if given, runs first on the vsync thread itself (to set its priority).
    VsyncThread(int32_t periodNanos, std::function<void(int64_t)> onVsync,
                std::function<void()> onStart = nullptr);
    ~VsyncThread();

    void setEnabled(bool enabled);

    // Ticks at `periodNanos` from the next tick on, when the display's refresh rate changes.
    void setPeriod(int32_t periodNanos);
    int32_t period() const { return mPeriodNanos.load(); }

   private:
    void run();

    std::atomic<int32_t> mPeriodNanos;
    const std::function<void(int64_t)> mOnVsync;
    const std::function<void()> mOnStart;
    std::mutex mMutex;
    std::condition_variable mWake;
    bool mEnabled = false;
    bool mStopping = false;
    std::thread mThread;
};

}  // namespace dsa::hwc
