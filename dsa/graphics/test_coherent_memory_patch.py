"""Tests for the guest Vulkan driver's coherent host-visible memory. A Hyper-V VM has no memory
the guest and the host share, so the host takes HOST_COHERENT off every memory type, and the
guest keeps its own copy of host-visible memory, sent to the host at each submit. Image test
run 42562909966 showed what that left: dEQP-VK.info.device_memory_properties failed ("required
property flags VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT|VK_MEMORY_PROPERTY_HOST_COHERENT_BIT not
found"), and an app that asks for coherent memory, as hello-vulkan does for its uniform
buffers, finds none."""

import re
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
PATCH_DIR = HERE / "patches/guest/mesa"
PATCH = next(PATCH_DIR.glob("0005-*.patch"))
TRACKER = "src/gfxstream/guest/vulkan_enc/ResourceTracker.cpp"
HEADER = "src/gfxstream/guest/vulkan_enc/ResourceTracker.h"
FUNC_TABLE = "src/gfxstream/guest/vulkan_enc/func_table.cpp"
TRANSFORM = "src/gfxstream/guest/vulkan_enc/goldfish_vk_transform_guest.cpp"


def file_hunks(patch, name):
    """The added and removed lines of one file in a patch."""
    text = patch.read_text(encoding="utf-8")
    for part in re.split(r"^diff --git ", text, flags=re.M):
        if part.startswith(f"a/{name} "):
            lines = part.splitlines()
            return ([l[1:] for l in lines if l.startswith("+") and not l.startswith("+++")],
                    [l[1:] for l in lines if l.startswith("-") and not l.startswith("---")])
    raise AssertionError(f"{patch.name} does not change {name}")


def function(added, signature):
    """The added lines of one function, from its signature to its closing brace."""
    start = next(i for i, line in enumerate(added) if line.startswith(signature))
    end = next(i for i in range(start, len(added)) if added[i] == "}")
    return "\n".join(added[start:end + 1])


class CoherentMemoryPatchTests(unittest.TestCase):
    def test_pr3_a_cached_host_visible_type_is_reported_coherent_without_shared_memory(self):
        added, removed = file_hunks(PATCH, TRANSFORM)
        self.assertFalse(removed)
        text = "\n".join(added)
        # Only where the guest keeps its own copy, and only for a type it reads back.
        self.assertIn("if (resourceTracker && !resourceTracker->usingDirectMapping()) {", text)
        self.assertIn("(flags & VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT) &&", text)
        self.assertIn("(flags & VK_MEMORY_PROPERTY_HOST_CACHED_BIT)", text)
        self.assertIn("flags |= VK_MEMORY_PROPERTY_HOST_COHERENT_BIT;", text)
        # The rule the read back follows is the same: a type the host caches.
        tracker = PATCH_DIR.parent.parent / "0003-copy-host-visible-memory-without-shared-memory.patch"
        self.assertIn("VK_MEMORY_PROPERTY_HOST_CACHED_BIT)) {", tracker.read_text(encoding="utf-8"))

    def test_pr3_every_way_an_app_learns_the_device_finished_reads_the_writes_back(self):
        added, _ = file_hunks(PATCH, FUNC_TABLE)
        text = "\n".join(added)
        # vkDeviceWaitIdle and vkWaitSemaphores, which the tracker does not see.
        self.assertEqual(text.count("ResourceTracker::get()->readBackAfterWait("), 2)
        self.assertIn("if (vkDeviceWaitIdle_VkResult_return == VK_SUCCESS) {", text)
        self.assertIn("if (vkWaitSemaphores_VkResult_return == VK_SUCCESS) {", text)
        self.assertIn("ResourceTracker::get()->readBackAfterSemaphoreValue(", text)

        added, removed = file_hunks(PATCH, TRACKER)
        text = "\n".join(added)
        # vkGetFenceStatus, from the host and from a sync fd, once the fence is signalled.
        self.assertIn("    return enc->vkGetFenceStatus(device, fence, /*doLock=*/true);", removed)
        self.assertIn("    if (result == VK_SUCCESS) readBackAfterFence(enc, device, fence);", added)
        self.assertIn("            lock.unlock();\n            readBackAfterFence(enc, device, fence);", text)
        # A submit is what a fence's next success reads back.
        self.assertIn("                fenceIt->second.submits++;", added)

    def test_pr3_a_polled_fence_or_semaphore_reads_back_once_per_completion(self):
        added, _ = file_hunks(PATCH, TRACKER)
        fence = function(added, "void ResourceTracker::readBackAfterFence(")
        self.assertIn("if (usingDirectMapping()) return;", fence)
        self.assertIn("it->second.readBackSubmits == it->second.submits", fence)
        # Marked only after the copies hold the writes.
        self.assertLess(fence.index("invalidateShadowedMemory(enc, device);"),
                        fence.index("it->second.readBackSubmits = submits;"))
        semaphore = function(added, "void ResourceTracker::readBackAfterSemaphoreValue(")
        self.assertIn("it->second.readBackValue >= value) return;", semaphore)
        self.assertLess(semaphore.index("invalidateShadowedMemory(enc, device);"),
                        semaphore.index("it->second.readBackValue = value;"))
        header, _ = file_hunks(PATCH, HEADER)
        self.assertIn("        uint64_t readBackSubmits = 0;", header)
        self.assertIn("        uint64_t readBackValue = 0;", header)

    def test_pr3_it_applies_after_the_patches_it_builds_on(self):
        names = sorted(p.name for p in PATCH_DIR.glob("*.patch"))
        # The shadow copy (patches/0003) and the CPU read back (guest/mesa/0002) are in its tree.
        self.assertEqual(names[names.index(PATCH.name) - 1][:5], "0004-")
        self.assertIn('for patch in "$patch_dir"/guest/mesa/*.patch; do',
                      (HERE / "fetch.sh").read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
