// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// How long the presentation waits for the GPU to copy a frame into a task window: never
// without an end, so a hung GPU cannot hold the renderer's device, and a copy that never
// finishes is told apart from one the GPU reset lost.

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <utility>
#include <vector>

#include "dsa_copy_wait.h"

namespace {

int failures = 0;

void check(bool ok, const char* test, const char* what) {
    if (!ok) {
        std::fprintf(stderr, "%s: %s\n", test, what);
        failures++;
    }
}

/* A fence that answers each wait in turn with the next of `answers`, and notes how long each
 * wait was allowed. */
struct FakeFence {
    explicit FakeFence(std::vector<dsa::FenceWait> given) : answers(std::move(given)) {}

    std::vector<dsa::FenceWait> answers;
    std::vector<uint64_t> timeouts;
    size_t next = 0;

    dsa::FenceWait operator()(uint64_t timeout_ns) {
        timeouts.push_back(timeout_ns);
        if (next < answers.size()) {
            return answers[next++];
        }
        return dsa::FenceWait::Timeout;
    }
};

uint64_t total(const FakeFence& fence) {
    uint64_t sum = 0;
    for (uint64_t timeout : fence.timeouts) {
        sum += timeout;
    }
    return sum;
}

void nfr5_a_copy_the_gpu_finishes_is_done_after_one_wait() {
    const char* test = __func__;
    FakeFence fence({dsa::FenceWait::Signaled});
    check(dsa::wait_for_copy(fence) == dsa::CopyEnd::Done, test, "done");
    check(fence.timeouts.size() == 1, test, "one wait");
}

void nfr5_a_slow_copy_is_waited_for_once_more() {
    const char* test = __func__;
    FakeFence fence({dsa::FenceWait::Timeout, dsa::FenceWait::Signaled});
    check(dsa::wait_for_copy(fence) == dsa::CopyEnd::Done, test, "done");
    check(fence.timeouts.size() == 2, test, "two waits");
}

void nfr5_a_copy_that_never_finishes_is_a_hung_gpu_and_the_wait_ends() {
    const char* test = __func__;
    FakeFence fence(std::vector<dsa::FenceWait>{});
    check(dsa::wait_for_copy(fence) == dsa::CopyEnd::Hung, test, "hung");
    check(fence.timeouts.size() == 2, test, "two waits, no more");
    for (uint64_t timeout : fence.timeouts) {
        check(timeout != UINT64_MAX, test, "no wait without an end");
    }
    check(total(fence) <= dsa::kCopyWaitNs + dsa::kCopyHungNs, test, "five seconds at most");
}

void nfr5_a_reset_gpu_is_a_lost_device_not_a_hang() {
    const char* test = __func__;
    FakeFence first({dsa::FenceWait::DeviceLost});
    check(dsa::wait_for_copy(first) == dsa::CopyEnd::DeviceLost, test, "lost at once");
    FakeFence later({dsa::FenceWait::Timeout, dsa::FenceWait::DeviceLost});
    check(dsa::wait_for_copy(later) == dsa::CopyEnd::DeviceLost, test, "lost while waiting");
}

void nfr5_a_failed_wait_never_lets_the_fence_be_used_again() {
    const char* test = __func__;
    FakeFence fence({dsa::FenceWait::Failed});
    check(dsa::wait_for_copy(fence) == dsa::CopyEnd::Hung, test, "left alone");
}

}  // namespace

int main() {
    nfr5_a_copy_the_gpu_finishes_is_done_after_one_wait();
    nfr5_a_slow_copy_is_waited_for_once_more();
    nfr5_a_copy_that_never_finishes_is_a_hung_gpu_and_the_wait_ends();
    nfr5_a_reset_gpu_is_a_lost_device_not_a_hang();
    nfr5_a_failed_wait_never_lets_the_fence_be_used_again();
    if (failures != 0) {
        std::fprintf(stderr, "%d check(s) failed\n", failures);
        return EXIT_FAILURE;
    }
    std::printf("copy wait: all checks passed\n");
    return EXIT_SUCCESS;
}
