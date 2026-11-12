// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The guest Vulkan driver's copy of host-visible memory: it takes no memory until written, and
// it is kept in step with the host only once the app maps it.

#include <sys/mman.h>
#include <unistd.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <vector>

#include "dsa_shadow_blocks.h"
#include "dsa_shadow_memory.h"

namespace {

using dsa::allocateShadowMemory;
using dsa::freeShadowMemory;
using dsa::ShadowMapping;

int failures = 0;

void check(bool ok, const char* test, const char* what) {
    if (!ok) {
        std::fprintf(stderr, "%s: %s\n", test, what);
        failures++;
    }
}

// How many pages of [memory, memory + size) the kernel has given memory to.
size_t residentPages(const uint8_t* memory, uint64_t size) {
    const uint64_t page = static_cast<uint64_t>(sysconf(_SC_PAGESIZE));
    const size_t pages = static_cast<size_t>((size + page - 1) / page);
#if defined(__APPLE__)
    std::vector<char> resident(pages, 0);
#else
    std::vector<unsigned char> resident(pages, 0);
#endif
    if (mincore(const_cast<uint8_t*>(memory), static_cast<size_t>(size), resident.data()) != 0) {
        return pages;  // Counted as all resident, so a failure to ask is not a pass.
    }
    size_t count = 0;
    for (auto byte : resident) count += (byte & 1) ? 1 : 0;
    return count;
}

void nfr1_a_copy_nothing_wrote_to_takes_no_guest_memory() {
    const char* test = __func__;
    // As large as a render target at 250%: the copy used to be filled with zeros at once.
    const uint64_t size = 64ull << 20;
    uint8_t* memory = allocateShadowMemory(size, 65536);
    check(memory != nullptr, test, "the copy is made");
    if (!memory) return;
    check(residentPages(memory, size) == 0, test, "no page is resident before anything writes");
    memory[12345] = 7;
    // One page, or one huge page where the kernel backs anonymous memory with them.
    const size_t written = residentPages(memory, size);
    check(written >= 1 && written <= (2u << 20) / static_cast<size_t>(sysconf(_SC_PAGESIZE)),
          test, "a write makes only the memory around it resident");
    freeShadowMemory(memory, size);
}

void nfr1_a_copy_is_aligned_and_starts_as_zero() {
    const char* test = __func__;
    for (uint64_t size : {uint64_t(1), uint64_t(4096), uint64_t(65536 + 3), uint64_t(3) << 20}) {
        uint8_t* memory = allocateShadowMemory(size, 65536);
        check(memory != nullptr, test, "the copy is made");
        if (!memory) continue;
        check(reinterpret_cast<uintptr_t>(memory) % 65536 == 0, test,
              "the copy starts at the alignment asked for");
        bool zero = true;
        for (uint64_t at = 0; at < size; at += 4093) zero = zero && memory[at] == 0;
        zero = zero && memory[size - 1] == 0;
        check(zero, test, "the copy reads as zero");
        // The hashes a new allocation starts with are of zeros: nothing to send.
        dsa::ShadowBlocks blocks(size);
        check(blocks.changed(memory).empty(), test, "an untouched copy has nothing to send");
        // The whole size can be written.
        memory[size - 1] = 1;
        check(blocks.changed(memory).size() == 1, test, "its last byte is part of it");
        freeShadowMemory(memory, size);
    }
}

void nfr1_freeing_a_copy_returns_its_pages() {
    const char* test = __func__;
    const uint64_t size = 8ull << 20;
    uint8_t* memory = allocateShadowMemory(size, 65536);
    check(memory != nullptr, test, "the copy is made");
    if (!memory) return;
    for (uint64_t at = 0; at < size; at += 4096) memory[at] = 1;
    freeShadowMemory(memory, size);
    // The range is no longer mapped: mincore refuses it.
#if defined(__APPLE__)
    std::vector<char> resident(1, 0);
#else
    std::vector<unsigned char> resident(1, 0);
#endif
    check(mincore(memory, 4096, resident.data()) != 0, test, "the freed copy is unmapped");
}

void nfr1_only_memory_the_app_mapped_is_kept_in_step() {
    const char* test = __func__;
    ShadowMapping mapping;
    check(!mapping.keptInStep(), test, "memory the app never mapped is neither sent nor read back");
    check(!mapping.takeFlush(), test, "memory the app never mapped is never flushed");
    check(mapping.firstMap(), test, "the first map reads the device's writes back");
    check(!mapping.keptInStep(), test, "not kept in step until the read back is done");
    mapping.mapped();
    check(mapping.keptInStep(), test, "a mapped allocation is kept in step");
    check(mapping.takeFlush(), test, "a mapped allocation flushes on every submit");
    check(mapping.takeFlush(), test, "while mapped, later submits keep flushing");
    check(!mapping.firstMap(), test, "while mapped, it is not treated as unmapped");

    // Unmapping flushes once at the next submit, then stops hashing and reading back.
    mapping.unmapped();
    check(!mapping.keptInStep(), test, "an unmapped allocation is not invalidated on waits");
    check(mapping.takeFlush(), test, "the first submit after unmap flushes pre-unmap writes");
    check(!mapping.takeFlush(), test, "later submits do not hash unmapped memory");
    check(!mapping.takeFlush(), test, "unmapped memory stays idle across frames");
    check(mapping.firstMap(), test, "remapping reads back device writes made while unmapped");

    mapping.mapped();
    check(mapping.keptInStep(), test, "remapping resumes keeping the copy in step");
    check(mapping.takeFlush(), test, "remapped memory flushes again");
}

}  // namespace

int main() {
    nfr1_a_copy_nothing_wrote_to_takes_no_guest_memory();
    nfr1_a_copy_is_aligned_and_starts_as_zero();
    nfr1_freeing_a_copy_returns_its_pages();
    nfr1_only_memory_the_app_mapped_is_kept_in_step();
    if (failures != 0) {
        std::fprintf(stderr, "%d failed\n", failures);
        return EXIT_FAILURE;
    }
    return EXIT_SUCCESS;
}
