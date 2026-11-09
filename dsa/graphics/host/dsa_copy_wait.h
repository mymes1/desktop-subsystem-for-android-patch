/*
 * Copyright 2026 The DSA Authors
 * SPDX-License-Identifier: Apache-2.0
 */

/* How long the presentation waits for the GPU to copy a frame into a task window.
 *
 * Each copy is submitted with a fence and waited for before the next frame of the display
 * reuses the command buffer. A copy that does not finish in a second is waited for once more,
 * up to four seconds, and no longer: Windows resets a GPU whose work runs past two seconds,
 * and a wait without an end would hold the renderer's device (and every task window's frames)
 * for as long as the GPU hangs. A copy still not finished then is taken as a hung GPU. Its
 * fence and command buffer are never reset or used again while the GPU may still run them;
 * the device is checked before the next operation and made again when it is lost. */

#pragma once

#include <cstdint>

namespace dsa {

/* The first wait, and the longest the second one may add. */
inline constexpr uint64_t kCopyWaitNs = 1000ull * 1000ull * 1000ull;
inline constexpr uint64_t kCopyHungNs = 4ull * 1000ull * 1000ull * 1000ull;

/* A copy slower than this is logged with its sizes, to find what slows the GPU down. */
inline constexpr uint64_t kCopySlowNs = 250ull * 1000ull * 1000ull;

/* What one wait for a fence gave. */
enum class FenceWait { Signaled, Timeout, DeviceLost, Failed };

/* How a copy ended. */
enum class CopyEnd {
    /* The GPU finished it: the fence may be reset and used again. */
    Done,
    /* The GPU was reset: the device is lost and made again before the next frame. */
    DeviceLost,
    /* The GPU has not finished it after both waits, or the wait failed: the fence and its
     * command buffer are left alone, and the device is suspected lost. */
    Hung,
};

/* Waits for a copy's fence with `wait(timeout_ns)`, which returns a FenceWait, at most twice
 * and never for longer than kCopyWaitNs + kCopyHungNs in all. */
template <typename Wait>
CopyEnd wait_for_copy(Wait&& wait) {
    FenceWait waited = wait(kCopyWaitNs);
    if (waited == FenceWait::Timeout) {
        waited = wait(kCopyHungNs);
    }
    switch (waited) {
        case FenceWait::Signaled:
            return CopyEnd::Done;
        case FenceWait::DeviceLost:
            return CopyEnd::DeviceLost;
        case FenceWait::Timeout:
        case FenceWait::Failed:
            break;
    }
    return CopyEnd::Hung;
}

}  // namespace dsa
