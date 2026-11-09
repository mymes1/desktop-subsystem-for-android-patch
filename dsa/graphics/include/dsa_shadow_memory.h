// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// The guest's copy of one host-visible allocation, and when it is kept in step with the host.
//
// A Hyper-V VM has no memory the guest and the host share, so the guest Vulkan driver keeps a
// copy of every allocation of a host-visible memory type. On such a host every memory type the
// guest sees is host visible, so every allocation an app makes has a copy, its images' included.
// Copying them all cost twice:
//
// - Each copy was filled with zeros when it was made. The guest then held as much memory again
//   as the app's whole GPU memory (textures, glyph atlases, render targets), which the CPU
//   never reads.
// - Every copy was sent to the host at each submit (as hashes compared block by block) and
//   read back whole from the host each time the app learnt its work had finished. An app's
//   renderer learns that every frame (Skia polls its fences), so each frame waited for all of
//   the app's GPU memory to cross the Hyper-V socket, and the renderer's stream held it too.
//
// Only memory the app has mapped can be read or written by the CPU. So a copy is made of pages
// the kernel hands out on first touch (an anonymous mapping, all zero), and it is kept in step
// only once the app maps it. On the first map a memory type the host caches is read back
// once, so the copy holds what the device wrote before (an uncached type the app invalidates
// itself, as before); from then on it is sent at every submit and read back as before.
// Memory the app never maps is never read back, sent or touched, and takes no guest memory.

#pragma once

#include <sys/mman.h>
#include <unistd.h>

#include <cstddef>
#include <cstdint>

namespace dsa {

// The bytes a copy of `size` bytes takes in whole pages, which is what it is mapped and
// unmapped as.
inline uint64_t shadowMemoryLength(uint64_t size) {
    const uint64_t page = static_cast<uint64_t>(sysconf(_SC_PAGESIZE));
    const uint64_t bytes = size == 0 ? 1 : size;
    return (bytes + page - 1) / page * page;
}

// A copy of `size` bytes at a multiple of `alignment` (a power of two): all zero, and taking
// no memory until something writes it. nullptr when the address space is short. Freed with
// freeShadowMemory and the same size.
inline uint8_t* allocateShadowMemory(uint64_t size, uint64_t alignment) {
    const uint64_t page = static_cast<uint64_t>(sysconf(_SC_PAGESIZE));
    if (alignment < page) alignment = page;
    const uint64_t length = shadowMemoryLength(size);
    // Enough to find an aligned start in; what lies outside the aligned part goes back.
    const uint64_t reserved = length + alignment - page;
    void* base = mmap(nullptr, static_cast<size_t>(reserved), PROT_READ | PROT_WRITE,
                      MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (base == MAP_FAILED) return nullptr;
    const uintptr_t start = reinterpret_cast<uintptr_t>(base);
    const uintptr_t aligned = (start + alignment - 1) & ~static_cast<uintptr_t>(alignment - 1);
    if (aligned > start) munmap(base, aligned - start);
    const uintptr_t end = aligned + length;
    const uintptr_t reservedEnd = start + reserved;
    if (reservedEnd > end) munmap(reinterpret_cast<void*>(end), reservedEnd - end);
    return reinterpret_cast<uint8_t*>(aligned);
}

inline void freeShadowMemory(uint8_t* memory, uint64_t size) {
    if (memory) munmap(memory, static_cast<size_t>(shadowMemoryLength(size)));
}

// Whether an allocation's copy is kept in step with the host: only once the app has mapped it.
// Unmapping does not end it, since what the app wrote before unmapping still goes to the host
// at the next submit.
class ShadowMapping {
   public:
    // Whether the app's next map is its first. Then the copy is read back from the host before
    // the app sees it, since the device may have written the memory while it was not kept in
    // step.
    bool firstMap() const { return !mMapped; }

    // The copy holds what the host has and the app has it mapped: from now on it is kept in
    // step. Marked only after the read back, so a submit meanwhile does not send the copy's
    // half-read contents to the host as if the app had written them.
    void mapped() { mMapped = true; }

    bool keptInStep() const { return mMapped; }

   private:
    bool mMapped = false;
};

}  // namespace dsa
