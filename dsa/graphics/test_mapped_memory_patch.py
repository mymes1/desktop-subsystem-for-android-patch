"""Tests for the guest Vulkan driver keeping only mapped memory in step with the host.

A Hyper-V VM has no memory the guest and the host share, so the driver keeps a copy of every
host-visible allocation, and on such a host every memory type the guest sees is host visible:
every allocation an app makes had a copy, its images' too. Each copy was filled with zeros
when it was made, and once that memory was reported coherent (patch 0005) every copy was read
back whole from the host each time the app learnt its work had finished, which Skia does
every frame: each frame waited for all of the app's GPU memory to cross the Hyper-V socket,
and the guest held a second copy of that memory as app memory.

Only memory the app maps can be read or written by the CPU, so now only that is kept in step
(dsa_shadow_memory.h, tested in tests/shadow_memory_test.cpp)."""

import re
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
PATCH_DIR = HERE / "patches/guest/mesa"
PATCH = next(PATCH_DIR.glob("0007-*.patch"))
TRACKER = "src/gfxstream/guest/vulkan_enc/ResourceTracker.cpp"
HEADER = "src/gfxstream/guest/vulkan_enc/ResourceTracker.h"


def file_hunks(patch, name):
    """The added and removed lines of one file in a patch."""
    text = patch.read_text(encoding="utf-8")
    for part in re.split(r"^diff --git ", text, flags=re.M):
        if part.startswith(f"a/{name} "):
            lines = part.splitlines()
            return ([l[1:] for l in lines if l.startswith("+") and not l.startswith("+++")],
                    [l[1:] for l in lines if l.startswith("-") and not l.startswith("---")])
    raise AssertionError(f"{patch.name} does not change {name}")


class MappedMemoryPatchTests(unittest.TestCase):
    def test_nfr1_memory_the_app_never_mapped_is_neither_sent_nor_read_back(self):
        added, removed = file_hunks(PATCH, TRACKER)
        text = "\n".join(added)
        # The flush before each submit and the read back after the app's waits both skip it.
        self.assertIn(
            "            if (!info.shadowed || info.device != device || !info.shadowBlocks) continue;",
            removed)
        self.assertIn(
            "            if (!info.shadowed || info.device != device) continue;", removed)
        self.assertEqual(text.count("!info.shadowMapping.keptInStep()"), 2)

    def test_nfr1_a_copy_is_made_of_zero_pages_not_filled_with_zeros(self):
        added, removed = file_hunks(PATCH, TRACKER)
        text = "\n".join(added)
        self.assertIn("        memset(shadow, 0, hostAllocationInfo.allocationSize);", removed)
        self.assertNotIn("memset(shadow", text)
        self.assertIn("dsa::allocateShadowMemory(hostAllocationInfo.allocationSize, kLargestPageSize)",
                      text)
        # Freed as it was made, with the size it was made with.
        self.assertIn("            free(info.ptr);", removed)
        self.assertIn("dsa::freeShadowMemory(info.ptr, info.shadowSize);", text)
        self.assertIn("stored.shadowSize = hostAllocationInfo.allocationSize;", text)

    def test_nfr1_the_first_map_reads_the_devices_writes_back_before_the_app_sees_them(self):
        added, _ = file_hunks(PATCH, TRACKER)
        text = "\n".join(added)
        start = text.index("if (deviceMemoryInfo.shadowed && deviceMemoryInfo.shadowMapping.firstMap())")
        block = text[start:]
        # Only a type the host caches, as every other read back.
        self.assertIn("VK_MEMORY_PROPERTY_HOST_CACHED_BIT", block)
        # Not under the tracker's lock, and only then is the copy kept in step.
        self.assertLess(block.index("lock.unlock();"), block.index("vkInvalidateMappedMemoryRanges("))
        self.assertLess(block.index("vkInvalidateMappedMemoryRanges("), block.index("lock.lock();"))
        self.assertLess(block.index("shadowBlocks->refresh(deviceMemoryInfo.ptr);"),
                        block.index("deviceMemoryInfo.shadowMapping.mapped();"))
        # Before the pointer goes to the app.
        raw = PATCH.read_text(encoding="utf-8")
        self.assertLess(raw.index("+        deviceMemoryInfo.shadowMapping.mapped();"),
                        raw.index(" *ppData = deviceMemoryInfo.ptr + offset;"))

    def test_nfr1_each_allocation_remembers_whether_it_was_mapped(self):
        added, _ = file_hunks(PATCH, HEADER)
        self.assertIn('#include "dsa_shadow_memory.h"', added)
        self.assertIn("        dsa::ShadowMapping shadowMapping;", added)
        self.assertIn("        uint64_t shadowSize = 0;", added)
        policy = (HERE / "include/dsa_shadow_memory.h").read_text(encoding="utf-8")
        self.assertIn("MAP_PRIVATE | MAP_ANONYMOUS", policy)
        self.assertIn("bool firstMap() const { return !mMapped; }", policy)

    def test_nfr1_it_applies_after_the_coherent_memory_patch_and_its_unit_test_runs(self):
        names = sorted(p.name for p in PATCH_DIR.glob("*.patch"))
        self.assertEqual(names[names.index(PATCH.name) - 1][:5], "0006-")
        self.assertIn('for patch in "$patch_dir"/guest/mesa/*.patch; do',
                      (HERE / "fetch.sh").read_text(encoding="utf-8"))
        tests = (HERE / "tests/test.sh").read_text(encoding="utf-8")
        self.assertIn('"$out/shadow_memory_test"', tests)

    def test_nfr1_unmapped_memory_flushes_once_and_stops_hashing_and_reading_back(self):
        unmap_patch = next(PATCH_DIR.glob("0011-*.patch"))
        added, _ = file_hunks(unmap_patch, TRACKER)
        text = "\n".join(added)
        self.assertIn("it->second.shadowMapping.unmapped();", text)
        self.assertIn("!info.shadowMapping.takeFlush()) {", text)
        policy = (HERE / "include/dsa_shadow_memory.h").read_text(encoding="utf-8")
        self.assertIn("void unmapped()", policy)
        self.assertIn("bool takeFlush()", policy)


if __name__ == "__main__":
    unittest.main()
