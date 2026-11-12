/*
 * Copyright 2026 The DSA Authors
 * SPDX-License-Identifier: Apache-2.0
 */

/* Per-display presentation: copies each display's newest color buffer into the Direct3D 11
 * texture the embedder (the compositor) names for it, on the renderer's own Vulkan queue.
 *
 * The color buffer stays at the largest size the task can have while its window is resized,
 * and the frame fills its top-left region, as large as the display's size (rcSetDisplayPose)
 * says. Only that region is copied, and the embedder is told its size.
 *
 * The guest sets a display's color buffer through the multi-display operations; this file
 * wraps that one operation so it also queues the frame for the presentation thread. The
 * thread keeps only the newest frame of each display, so a slow window never makes the
 * guest wait and never shows a stale frame after a newer one. */

#include "dsa_gfx_backend.h"
#include "dsa_frame_crop.h"
#include "dsa_gfx_display.h"

#include <algorithm>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstring>
#include <map>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include "FrameBuffer.h"
#include "dsa_copy_bands.h"
#include "dsa_copy_wait.h"
#include "dsa_device_remake.h"
#include "host-common/logging.h"
#include "host-common/misc.h"
#include "host-common/multi_display_agent.h"
#include "vulkan/BorrowedImageVk.h"
#include "vulkan/VkCommonOperations.h"

namespace {

/* The generation the presentation follows while the renderer's device is being made again:
 * none, so the objects made on the old device are forgotten. */
constexpr uint64_t kNoDevice = ~uint64_t{0};

/* Waits for the copy `fence` follows, never without an end (dsa_copy_wait.h), and says how
 * long it took in `took_ns`. A finished copy's fence is reset for the next one; a hung one's
 * is left alone, and the device is suspected lost, so the renderer checks it before its next
 * operation. */
dsa::CopyEnd finish_copy(gfxstream::vk::VkEmulation& emu, VkFence fence, uint64_t* took_ns) {
    gfxstream::vk::VulkanDispatch* vk = emu.getDeviceDispatch();
    VkDevice device = emu.getDevice();
    const auto started = std::chrono::steady_clock::now();
    /* In slices, so a loss another thread found ends the wait at once: the device lock this
     * thread holds is what making the device again waits for. */
    const dsa::CopyEnd end = dsa::wait_for_copy([&](uint64_t timeout_ns) {
        const dsa::GpuWait waited = dsa::wait_bounded(
            [&](uint64_t slice_ns) {
                switch (vk->vkWaitForFences(device, 1, &fence, VK_TRUE, slice_ns)) {
                    case VK_SUCCESS:
                        return dsa::GpuWait::Signaled;
                    case VK_TIMEOUT:
                        return dsa::GpuWait::Timeout;
                    case VK_ERROR_DEVICE_LOST:
                        return dsa::GpuWait::DeviceLost;
                    default:
                        return dsa::GpuWait::Failed;
                }
            },
            [&] { return !emu.deviceUsable(); }, timeout_ns);
        switch (waited) {
            case dsa::GpuWait::Signaled:
                return dsa::FenceWait::Signaled;
            case dsa::GpuWait::Timeout:
                return dsa::FenceWait::Timeout;
            case dsa::GpuWait::DeviceLost:
            case dsa::GpuWait::GivenUp:
                return dsa::FenceWait::DeviceLost;
            case dsa::GpuWait::Failed:
                break;
        }
        return dsa::FenceWait::Failed;
    });
    *took_ns = static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::nanoseconds>(
                                         std::chrono::steady_clock::now() - started)
                                         .count());
    switch (end) {
        case dsa::CopyEnd::Done:
            vk->vkResetFences(device, 1, &fence);
            break;
        case dsa::CopyEnd::DeviceLost:
            emu.noteDeviceLost();
            break;
        case dsa::CopyEnd::Hung:
            emu.suspectDeviceLost();
            break;
    }
    return end;
}

/* Whether a slow copy may be logged now: at most one line every two seconds, so a GPU that
 * stays slow does not fill the log. */
[[maybe_unused]] bool slow_copy_may_log() {
    static std::mutex mutex;
    static std::chrono::steady_clock::time_point last;
    std::lock_guard<std::mutex> lock(mutex);
    const auto now = std::chrono::steady_clock::now();
    if (last.time_since_epoch().count() != 0 && now - last < std::chrono::seconds(2)) {
        return false;
    }
    last = now;
    return true;
}

}  // namespace

#ifdef _WIN32

#include <d3d11_1.h>
#include <dxgi1_2.h>
#include <wrl/client.h>

namespace {

using gfxstream::FrameBuffer;
using gfxstream::vk::BorrowedImageInfoVk;
using gfxstream::vk::VkEmulation;
using gfxstream::vk::VulkanDispatch;
using Microsoft::WRL::ComPtr;

struct D3d11SharedDevice {
    ComPtr<ID3D11Device1> device;
    ComPtr<ID3D11DeviceContext> context;
    ComPtr<ID3D11Query> event_query;
};

/* A texture of the embedder's, either imported into Vulkan or opened through Direct3D 11.1
 * when the Vulkan driver cannot import Direct3D 11 handles directly (e.g. Lavapipe or a
 * cross-adapter render GPU). */
struct Target {
    void* handle = nullptr;
    uint32_t width = 0;
    uint32_t height = 0;
    VkImage image = VK_NULL_HANDLE;
    VkDeviceMemory memory = VK_NULL_HANDLE;
    VkBuffer staging_buffer = VK_NULL_HANDLE;
    VkDeviceMemory staging_memory = VK_NULL_HANDLE;
    void* staging_mapped = nullptr;
    VkDeviceSize staging_size = 0;
    bool staging_coherent = false;
    VkImage convert_image = VK_NULL_HANDLE;
    VkDeviceMemory convert_memory = VK_NULL_HANDLE;
    ComPtr<ID3D11Texture2D> d3d_texture;
    size_t d3d_device_index = 0;
};

/* A frame waiting to be shown: its color buffer, and the display's size when it was set
 * (0 when the display has none). */
struct PendingFrame {
    uint32_t color_buffer = 0;
    uint32_t width = 0;
    uint32_t height = 0;
};

struct Presenter {
    std::mutex mutex;
    std::condition_variable wake;
    /* The newest frame set for each display, waiting to be shown. */
    std::map<uint32_t, PendingFrame> pending;
    /* Displays that lost their window: their imported textures go on the next pass. */
    std::vector<uint32_t> forget;
    bool stopping = false;
    std::thread thread;

    dsa_display_target_fn target_cb = nullptr;
    dsa_display_frame_fn frame_cb = nullptr;
    void* context = nullptr;

    /* Only the presentation thread touches these. */
    std::map<uint32_t, Target> targets;
    std::vector<D3d11SharedDevice> d3d_devices;
    bool d3d_devices_probed = false;
    VkCommandPool pool = VK_NULL_HANDLE;
    VkCommandBuffer commands = VK_NULL_HANDLE;
    VkFence fence = VK_NULL_HANDLE;
    /* The renderer's device these objects were made on (VkEmulation::deviceGeneration). */
    uint64_t generation = 0;
};

std::mutex s_presenter_mutex;
Presenter* s_presenter = nullptr;
int (*s_original_set_display_color_buffer)(uint32_t, uint32_t) = nullptr;
int (*s_get_display_pose)(uint32_t, int32_t*, int32_t*, uint32_t*, uint32_t*) = nullptr;
bool s_hook_installed = false;

void log_once(const char* what) {
    static std::mutex log_mutex;
    static std::vector<std::string> said;
    std::lock_guard<std::mutex> lock(log_mutex);
    for (const auto& line : said) {
        if (line == what) {
            return;
        }
    }
    said.emplace_back(what);
    std::fprintf(stderr, "dsa_gfx: %s\n", what);
}

VkEmulation* emulation() {
    FrameBuffer* fb = FrameBuffer::getFB();
    if (!fb || !fb->hasEmulationVk()) {
        return nullptr;
    }
    return &fb->getEmulationVk();
}

int hooked_set_display_color_buffer(uint32_t display_id, uint32_t color_buffer) {
    int result = s_original_set_display_color_buffer
                     ? s_original_set_display_color_buffer(display_id, color_buffer)
                     : 0;
    if (color_buffer == 0) {
        return result;
    }
    /* Read here, on the thread that decodes the guest's commands, so the size is the one the
     * guest set just before this frame. */
    PendingFrame frame;
    frame.color_buffer = color_buffer;
    if (s_get_display_pose) {
        int32_t x = 0;
        int32_t y = 0;
        uint32_t w = 0;
        uint32_t h = 0;
        if (s_get_display_pose(display_id, &x, &y, &w, &h) == 0) {
            frame.width = w;
            frame.height = h;
        }
    }
    std::lock_guard<std::mutex> lock(s_presenter_mutex);
    if (s_presenter) {
        std::lock_guard<std::mutex> pending_lock(s_presenter->mutex);
        s_presenter->pending[display_id] = frame;
        s_presenter->wake.notify_one();
    }
    return result;
}

/* Wraps the multi-display operation that sets a display's color buffer, once. */
bool install_hook() {
    if (s_hook_installed) {
        return true;
    }
    QAndroidMultiDisplayAgent operations = emugl::get_emugl_multi_display_operations();
    s_original_set_display_color_buffer = operations.setDisplayColorBuffer;
    s_get_display_pose = operations.getDisplayPose;
    operations.setDisplayColorBuffer = hooked_set_display_color_buffer;
    emugl::set_emugl_multi_display_operations(operations);
    s_hook_installed = true;
    return true;
}

void destroy_target(VkEmulation& emu, Target& target) {
    VulkanDispatch* vk = emu.getDeviceDispatch();
    VkDevice device = emu.getDevice();
    if (target.staging_mapped && target.staging_memory != VK_NULL_HANDLE) {
        vk->vkUnmapMemory(device, target.staging_memory);
        target.staging_mapped = nullptr;
    }
    if (target.staging_buffer != VK_NULL_HANDLE) {
        vk->vkDestroyBuffer(device, target.staging_buffer, nullptr);
    }
    if (target.staging_memory != VK_NULL_HANDLE) {
        vk->vkFreeMemory(device, target.staging_memory, nullptr);
    }
    if (target.convert_image != VK_NULL_HANDLE) {
        vk->vkDestroyImage(device, target.convert_image, nullptr);
    }
    if (target.convert_memory != VK_NULL_HANDLE) {
        vk->vkFreeMemory(device, target.convert_memory, nullptr);
    }
    if (target.image != VK_NULL_HANDLE) {
        vk->vkDestroyImage(device, target.image, nullptr);
    }
    if (target.memory != VK_NULL_HANDLE) {
        vk->vkFreeMemory(device, target.memory, nullptr);
    }
    target = Target{};
}

bool find_memory_type(VulkanDispatch* ivk, VkPhysicalDevice phys, uint32_t type_bits,
                      VkMemoryPropertyFlags wanted, uint32_t* out_index, bool* out_coherent) {
    VkPhysicalDeviceMemoryProperties props = {};
    ivk->vkGetPhysicalDeviceMemoryProperties(phys, &props);
    for (uint32_t i = 0; i < props.memoryTypeCount; ++i) {
        if ((type_bits & (1u << i)) == 0) {
            continue;
        }
        const VkMemoryPropertyFlags flags = props.memoryTypes[i].propertyFlags;
        if ((flags & wanted) == wanted) {
            *out_index = i;
            if (out_coherent) {
                *out_coherent = (flags & VK_MEMORY_PROPERTY_HOST_COHERENT_BIT) != 0;
            }
            return true;
        }
    }
    return false;
}

void ensure_d3d11_devices(Presenter& presenter) {
    if (presenter.d3d_devices_probed) {
        return;
    }
    presenter.d3d_devices_probed = true;

    const D3D_FEATURE_LEVEL levels[] = {
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    };

    auto try_add_device = [&presenter, &levels](IDXGIAdapter1* adapter, D3D_DRIVER_TYPE type) {
        ComPtr<ID3D11Device> base_device;
        ComPtr<ID3D11DeviceContext> context;
        D3D_FEATURE_LEVEL chosen = D3D_FEATURE_LEVEL_11_0;
        HRESULT hr = D3D11CreateDevice(adapter, type, nullptr, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                                       levels, static_cast<UINT>(sizeof(levels) / sizeof(levels[0])),
                                       D3D11_SDK_VERSION, base_device.GetAddressOf(), &chosen,
                                       context.GetAddressOf());
        if (FAILED(hr) || !base_device || !context) {
            return;
        }
        ComPtr<ID3D11Device1> device1;
        if (FAILED(base_device.As(&device1)) || !device1) {
            return;
        }
        D3D11_QUERY_DESC query_desc = {};
        query_desc.Query = D3D11_QUERY_EVENT;
        ComPtr<ID3D11Query> query;
        if (FAILED(device1->CreateQuery(&query_desc, query.GetAddressOf())) || !query) {
            return;
        }
        D3d11SharedDevice entry;
        entry.device = std::move(device1);
        entry.context = std::move(context);
        entry.event_query = std::move(query);
        presenter.d3d_devices.push_back(std::move(entry));
    };

    ComPtr<IDXGIFactory1> factory;
    if (SUCCEEDED(CreateDXGIFactory1(__uuidof(IDXGIFactory1),
                                     reinterpret_cast<void**>(factory.GetAddressOf()))) &&
        factory) {
        for (UINT idx = 0;; ++idx) {
            ComPtr<IDXGIAdapter1> adapter;
            if (factory->EnumAdapters1(idx, adapter.GetAddressOf()) == DXGI_ERROR_NOT_FOUND) {
                break;
            }
            DXGI_ADAPTER_DESC1 desc = {};
            if (SUCCEEDED(adapter->GetDesc1(&desc)) &&
                (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) != 0) {
                continue;
            }
            try_add_device(adapter.Get(), D3D_DRIVER_TYPE_UNKNOWN);
        }
    }
    try_add_device(nullptr, D3D_DRIVER_TYPE_WARP);
}

bool open_d3d11_shared_target(VkEmulation& emu, Presenter& presenter, void* handle, uint32_t width,
                              uint32_t height, Target* out);
bool ensure_convert_image(VkEmulation& emu, Target& target);

bool open_d3d11_shared_target(VkEmulation& emu, Presenter& presenter, void* handle, uint32_t width,
                              uint32_t height, Target* out) {
    ensure_d3d11_devices(presenter);

    ComPtr<ID3D11Texture2D> shared_texture;
    size_t matched_device = 0;
    bool opened = false;
    for (size_t i = 0; i < presenter.d3d_devices.size(); ++i) {
        ComPtr<ID3D11Texture2D> candidate;
        HRESULT hr = presenter.d3d_devices[i].device->OpenSharedResource1(
            static_cast<HANDLE>(handle), __uuidof(ID3D11Texture2D),
            reinterpret_cast<void**>(candidate.GetAddressOf()));
        if (SUCCEEDED(hr) && candidate) {
            D3D11_TEXTURE2D_DESC desc = {};
            candidate->GetDesc(&desc);
            if (desc.Width >= width && desc.Height >= height &&
                desc.Format == DXGI_FORMAT_R8G8B8A8_UNORM) {
                shared_texture = std::move(candidate);
                matched_device = i;
                opened = true;
                break;
            }
        }
    }
    if (!opened) {
        log_once("could not open a task window's shared Direct3D 11 texture for staging");
        return false;
    }

    VulkanDispatch* vk = emu.getDeviceDispatch();
    VulkanDispatch* ivk = emu.getInstanceDispatch();
    VkDevice device = emu.getDevice();
    VkPhysicalDevice phys = emu.getPhysicalDevice();
    const VkDeviceSize byte_size =
        static_cast<VkDeviceSize>(width) * static_cast<VkDeviceSize>(height) * 4u;

    Target target;
    target.handle = handle;
    target.width = width;
    target.height = height;
    target.staging_size = byte_size;
    target.d3d_texture = std::move(shared_texture);
    target.d3d_device_index = matched_device;

    VkBufferCreateInfo buf_info = {};
    buf_info.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
    buf_info.size = byte_size;
    buf_info.usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT;
    buf_info.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    if (vk->vkCreateBuffer(device, &buf_info, nullptr, &target.staging_buffer) != VK_SUCCESS) {
        destroy_target(emu, target);
        return false;
    }

    VkMemoryRequirements reqs = {};
    vk->vkGetBufferMemoryRequirements(device, target.staging_buffer, &reqs);
    uint32_t type_index = 0;
    bool coherent = false;
    if (!find_memory_type(ivk, phys, reqs.memoryTypeBits,
                          VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_CACHED_BIT,
                          &type_index, &coherent) &&
        !find_memory_type(ivk, phys, reqs.memoryTypeBits,
                          VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT |
                              VK_MEMORY_PROPERTY_HOST_COHERENT_BIT,
                          &type_index, &coherent) &&
        !find_memory_type(ivk, phys, reqs.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT,
                          &type_index, &coherent)) {
        destroy_target(emu, target);
        return false;
    }
    target.staging_coherent = coherent;

    VkMemoryAllocateInfo alloc_info = {};
    alloc_info.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    alloc_info.allocationSize = reqs.size;
    alloc_info.memoryTypeIndex = type_index;
    if (vk->vkAllocateMemory(device, &alloc_info, nullptr, &target.staging_memory) != VK_SUCCESS) {
        destroy_target(emu, target);
        return false;
    }
    if (vk->vkBindBufferMemory(device, target.staging_buffer, target.staging_memory, 0) !=
        VK_SUCCESS) {
        destroy_target(emu, target);
        return false;
    }
    if (vk->vkMapMemory(device, target.staging_memory, 0, reqs.size, 0, &target.staging_mapped) !=
            VK_SUCCESS ||
        !target.staging_mapped) {
        destroy_target(emu, target);
        return false;
    }
    if (!ensure_convert_image(emu, target)) {
        destroy_target(emu, target);
        return false;
    }

    log_once("using Direct3D 11 shared-texture staging because Vulkan could not import the "
             "task window's texture directly");
    *out = std::move(target);
    return true;
}

bool ensure_convert_image(VkEmulation& emu, Target& target) {
    if (target.convert_image != VK_NULL_HANDLE) {
        return true;
    }
    VulkanDispatch* vk = emu.getDeviceDispatch();
    VulkanDispatch* ivk = emu.getInstanceDispatch();
    VkDevice device = emu.getDevice();
    VkPhysicalDevice phys = emu.getPhysicalDevice();

    VkImageCreateInfo info = {};
    info.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
    info.imageType = VK_IMAGE_TYPE_2D;
    info.format = VK_FORMAT_R8G8B8A8_UNORM;
    info.extent = {target.width, target.height, 1};
    info.mipLevels = 1;
    info.arrayLayers = 1;
    info.samples = VK_SAMPLE_COUNT_1_BIT;
    info.tiling = VK_IMAGE_TILING_OPTIMAL;
    info.usage = VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
    info.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    info.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
    if (vk->vkCreateImage(device, &info, nullptr, &target.convert_image) != VK_SUCCESS) {
        return false;
    }

    VkMemoryRequirements reqs = {};
    vk->vkGetImageMemoryRequirements(device, target.convert_image, &reqs);
    uint32_t type_index = 0;
    if (!find_memory_type(ivk, phys, reqs.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT,
                          &type_index, nullptr) &&
        !find_memory_type(ivk, phys, reqs.memoryTypeBits, 0, &type_index, nullptr)) {
        vk->vkDestroyImage(device, target.convert_image, nullptr);
        target.convert_image = VK_NULL_HANDLE;
        return false;
    }

    VkMemoryAllocateInfo alloc_info = {};
    alloc_info.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    alloc_info.allocationSize = reqs.size;
    alloc_info.memoryTypeIndex = type_index;
    if (vk->vkAllocateMemory(device, &alloc_info, nullptr, &target.convert_memory) != VK_SUCCESS) {
        vk->vkDestroyImage(device, target.convert_image, nullptr);
        target.convert_image = VK_NULL_HANDLE;
        return false;
    }
    if (vk->vkBindImageMemory(device, target.convert_image, target.convert_memory, 0) !=
        VK_SUCCESS) {
        vk->vkFreeMemory(device, target.convert_memory, nullptr);
        vk->vkDestroyImage(device, target.convert_image, nullptr);
        target.convert_memory = VK_NULL_HANDLE;
        target.convert_image = VK_NULL_HANDLE;
        return false;
    }
    return true;
}

/* Imports the embedder's texture as a Vulkan image the copy can write. */
bool import_target(VkEmulation& emu, void* handle, uint32_t width, uint32_t height,
                   Target* out) {
    VulkanDispatch* vk = emu.getDeviceDispatch();
    VkDevice device = emu.getDevice();
    if (!vk->vkGetMemoryWin32HandlePropertiesKHR) {
        log_once("the Vulkan driver does not expose VK_KHR_external_memory_win32; falling back to "
                 "Direct3D 11 shared-texture staging");
        return false;
    }
    const VkExternalMemoryHandleTypeFlagBits handle_type =
        VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_BIT;

    VkExternalMemoryImageCreateInfo external_info = {};
    external_info.sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO;
    external_info.handleTypes = handle_type;

    VkImageCreateInfo image_info = {};
    image_info.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
    image_info.pNext = &external_info;
    image_info.imageType = VK_IMAGE_TYPE_2D;
    image_info.format = VK_FORMAT_R8G8B8A8_UNORM;
    image_info.extent = {width, height, 1};
    image_info.mipLevels = 1;
    image_info.arrayLayers = 1;
    image_info.samples = VK_SAMPLE_COUNT_1_BIT;
    image_info.tiling = VK_IMAGE_TILING_OPTIMAL;
    image_info.usage = VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
    image_info.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    image_info.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;

    Target target;
    target.handle = handle;
    target.width = width;
    target.height = height;
    if (vk->vkCreateImage(device, &image_info, nullptr, &target.image) != VK_SUCCESS) {
        log_once("could not create a Vulkan image for a task window's texture");
        return false;
    }

    VkMemoryWin32HandlePropertiesKHR handle_properties = {};
    handle_properties.sType = VK_STRUCTURE_TYPE_MEMORY_WIN32_HANDLE_PROPERTIES_KHR;
    if (vk->vkGetMemoryWin32HandlePropertiesKHR(device, handle_type, handle,
                                                &handle_properties) != VK_SUCCESS) {
        log_once("the Vulkan driver refused a task window's Direct3D 11 texture");
        destroy_target(emu, target);
        return false;
    }
    VkMemoryRequirements requirements = {};
    vk->vkGetImageMemoryRequirements(device, target.image, &requirements);
    uint32_t type_bits = requirements.memoryTypeBits & handle_properties.memoryTypeBits;
    if (type_bits == 0) {
        type_bits = handle_properties.memoryTypeBits;
    }
    if (type_bits == 0) {
        log_once("no Vulkan memory type can hold a task window's texture");
        destroy_target(emu, target);
        return false;
    }
    uint32_t type_index = 0;
    while (!(type_bits & (1u << type_index))) {
        type_index++;
    }

    VkMemoryDedicatedAllocateInfo dedicated = {};
    dedicated.sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO;
    dedicated.image = target.image;

    VkImportMemoryWin32HandleInfoKHR import_info = {};
    import_info.sType = VK_STRUCTURE_TYPE_IMPORT_MEMORY_WIN32_HANDLE_INFO_KHR;
    import_info.pNext = &dedicated;
    import_info.handleType = handle_type;
    import_info.handle = static_cast<HANDLE>(handle);

    VkMemoryAllocateInfo allocate_info = {};
    allocate_info.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    allocate_info.pNext = &import_info;
    allocate_info.allocationSize = requirements.size;
    allocate_info.memoryTypeIndex = type_index;
    if (vk->vkAllocateMemory(device, &allocate_info, nullptr, &target.memory) != VK_SUCCESS) {
        log_once("could not import a task window's Direct3D 11 texture into Vulkan");
        destroy_target(emu, target);
        return false;
    }
    if (vk->vkBindImageMemory(device, target.image, target.memory, 0) != VK_SUCCESS) {
        log_once("could not bind a task window's texture to its Vulkan image");
        destroy_target(emu, target);
        return false;
    }
    *out = target;
    return true;
}

/* The renderer made a new device after a GPU reset: everything made on the old one went with
 * it, so it is forgotten, not destroyed, and made again on the new one as frames come. */
void follow_device(VkEmulation& emu, Presenter& presenter) {
    /* A device being made again may be gone already: nothing is destroyed on it, and the
     * objects are made again on the new one. */
    const uint64_t generation = emu.deviceUsable() ? emu.deviceGeneration() : kNoDevice;
    if (presenter.generation == generation) {
        return;
    }
    presenter.generation = generation;
    presenter.targets.clear();
    presenter.pool = VK_NULL_HANDLE;
    presenter.commands = VK_NULL_HANDLE;
    presenter.fence = VK_NULL_HANDLE;
    log_once("the renderer made a new Vulkan device after a GPU reset; task windows follow it");
}

bool ensure_commands(VkEmulation& emu, Presenter& presenter) {
    if (presenter.commands != VK_NULL_HANDLE) {
        return true;
    }
    VulkanDispatch* vk = emu.getDeviceDispatch();
    VkDevice device = emu.getDevice();
    VkCommandPoolCreateInfo pool_info = {};
    pool_info.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO;
    pool_info.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    pool_info.queueFamilyIndex = emu.getQueueFamilyIndex();
    if (vk->vkCreateCommandPool(device, &pool_info, nullptr, &presenter.pool) != VK_SUCCESS) {
        return false;
    }
    VkCommandBufferAllocateInfo buffer_info = {};
    buffer_info.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
    buffer_info.commandPool = presenter.pool;
    buffer_info.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    buffer_info.commandBufferCount = 1;
    if (vk->vkAllocateCommandBuffers(device, &buffer_info, &presenter.commands) != VK_SUCCESS) {
        return false;
    }
    VkFenceCreateInfo fence_info = {};
    fence_info.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO;
    if (vk->vkCreateFence(device, &fence_info, nullptr, &presenter.fence) != VK_SUCCESS) {
        return false;
    }
    return true;
}

VkImageMemoryBarrier image_barrier(VkImage image, VkAccessFlags src_access,
                                   VkAccessFlags dst_access, VkImageLayout old_layout,
                                   VkImageLayout new_layout, uint32_t src_family,
                                   uint32_t dst_family) {
    VkImageMemoryBarrier barrier = {};
    barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
    barrier.srcAccessMask = src_access;
    barrier.dstAccessMask = dst_access;
    barrier.oldLayout = old_layout;
    barrier.newLayout = new_layout;
    barrier.srcQueueFamilyIndex = src_family;
    barrier.dstQueueFamilyIndex = dst_family;
    barrier.image = image;
    barrier.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    return barrier;
}

void cmd_barriers(VulkanDispatch* vk, VkCommandBuffer commands, VkPipelineStageFlags src,
                  VkPipelineStageFlags dst, const std::vector<VkImageMemoryBarrier>& barriers) {
    if (barriers.empty()) {
        return;
    }
    vk->vkCmdPipelineBarrier(commands, src, dst, 0, 0, nullptr, 0, nullptr,
                             static_cast<uint32_t>(barriers.size()), barriers.data());
}

/* Copies the part of `frame`'s color buffer the frame fills into the display's texture and
 * tells the embedder. */
void present(Presenter& presenter, uint32_t display_id, const PendingFrame& frame) {
    const uint32_t color_buffer = frame.color_buffer;
    VkEmulation* emu = emulation();
    FrameBuffer* fb = FrameBuffer::getFB();
    if (!emu || !fb) {
        return;
    }
    /* A lost device is made again on a thread of the renderer's own, which logs how long it
     * took; until then the frame is dropped at once, as the guest's calls fail at once. None
     * is made while the copy uses the device. */
    if (!emu->recoverIfLost()) {
        return;
    }
    auto device_use = emu->useDevice();
    follow_device(*emu, presenter);
    /* Held for the copy, so the guest destroying the buffer meanwhile cannot free its image. */
    auto keep_alive = fb->findColorBuffer(color_buffer);
    if (!keep_alive) {
        return;
    }
    const uint32_t width = keep_alive->getWidth();
    const uint32_t height = keep_alive->getHeight();
    if (width == 0 || height == 0) {
        return;
    }

    void* handle = presenter.target_cb(presenter.context, display_id, width, height);
    Target& target = presenter.targets[display_id];
    if (handle != target.handle || width != target.width || height != target.height) {
        destroy_target(*emu, target);
    }
    if (!handle) {
        presenter.targets.erase(display_id);
        return;
    }
    if (target.image == VK_NULL_HANDLE && !target.d3d_texture) {
        if (!import_target(*emu, handle, width, height, &target) &&
            !open_d3d11_shared_target(*emu, presenter, handle, width, height, &target)) {
            presenter.targets.erase(display_id);
            return;
        }
        /* Once per texture: a resize that remakes the guest's buffers remakes it, so the log
         * shows each size a window's frames were copied at. */
        WARN("Display %u's frames go to a new %ux%u task window texture (frame %ux%u, %s).",
             display_id, width, height, frame.width, frame.height,
             target.d3d_texture ? "d3d11-staged" : "vulkan-imported");
    }
    if (!ensure_commands(*emu, presenter)) {
        log_once("could not set up the Vulkan commands that copy frames into task windows");
        return;
    }
    /* Borrowed last: borrowing records the layout the copy leaves the buffer in, so from
     * here on the copy must be submitted. */
    std::unique_ptr<BorrowedImageInfoVk> source = emu->borrowColorBufferForDisplay(color_buffer);
    if (!source) {
        return;
    }
    const uint32_t image_width = source->imageCreateInfo.extent.width;
    const uint32_t image_height = source->imageCreateInfo.extent.height;
    /* The region the frame fills: the display's size, clipped to the buffer, the texture and
     * the image. A display without a size fills the whole buffer. */
    uint32_t content_width = frame.width ? frame.width : width;
    uint32_t content_height = frame.height ? frame.height : height;
    content_width = std::min(std::min(content_width, width), image_width);
    content_height = std::min(std::min(content_height, height), image_height);
    if (content_width == 0 || content_height == 0) {
        return;
    }
    const int32_t copy_width = static_cast<int32_t>(content_width);

    const bool use_d3d_staging = static_cast<bool>(target.d3d_texture);
    const bool source_is_rgba8 =
        source->imageCreateInfo.format == VK_FORMAT_R8G8B8A8_UNORM ||
        source->imageCreateInfo.format == VK_FORMAT_R8G8B8A8_SRGB;

    if (image_width != width || image_height != height) {
        /* The buffer and its Vulkan image disagree on the size: the copy is cut to the smaller,
         * and the line says so, since a mismatch is one way a copy can reach past an image. */
        WARN("ColorBuffer %u of display %u is %ux%u, but its Vulkan image is %ux%u.",
             color_buffer, display_id, width, height, image_width, image_height);
    }

    VulkanDispatch* vk = emu->getDeviceDispatch();
    const uint32_t family = emu->getQueueFamilyIndex();

    std::vector<VkImageMemoryBarrier> acquire_transfers;
    std::vector<VkImageMemoryBarrier> acquire_layouts;
    std::vector<VkImageMemoryBarrier> release_layouts;
    std::vector<VkImageMemoryBarrier> release_transfers;
    gfxstream::vk::addNeededBarriersToUseBorrowedImage(
        *source, family, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_ACCESS_TRANSFER_READ_BIT, &acquire_transfers,
        &acquire_layouts, &release_layouts, &release_transfers);

    if (!use_d3d_staging) {
        /* The texture's earlier contents do not matter; it comes from and goes back to
         * Direct3D. */
        acquire_layouts.push_back(image_barrier(
            target.image, 0, VK_ACCESS_TRANSFER_WRITE_BIT, VK_IMAGE_LAYOUT_UNDEFINED,
            VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, VK_QUEUE_FAMILY_IGNORED,
            VK_QUEUE_FAMILY_IGNORED));
        release_transfers.push_back(image_barrier(
            target.image, VK_ACCESS_TRANSFER_WRITE_BIT, 0, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
            VK_IMAGE_LAYOUT_GENERAL, family, VK_QUEUE_FAMILY_EXTERNAL));
    }

    /* The copy goes in bands of at most dsa::kMaxCopyPixels, each a submission of its own
     * that is waited for before the next: a frame of a window maximized on a 200% monitor
     * (2880x1800 and more) is never one GPU job, and the GPU can turn to other work between
     * the bands. The first band takes the images over, the last hands them back. */
    const std::vector<dsa::CopyBand> bands = dsa::copy_bands(content_width, content_height);
    uint64_t took_ns = 0;
    for (size_t band = 0; band < bands.size(); ++band) {
        const bool first = band == 0;
        const bool last = band + 1 == bands.size();
        const uint32_t band_y = bands[band].y;
        const uint32_t band_rows = bands[band].rows;
        const int32_t top = static_cast<int32_t>(band_y);
        const int32_t bottom = top + static_cast<int32_t>(band_rows);

        VkCommandBufferBeginInfo begin = {};
        begin.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
        begin.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
        vk->vkResetCommandBuffer(presenter.commands, 0);
        vk->vkBeginCommandBuffer(presenter.commands, &begin);
        if (first) {
            cmd_barriers(vk, presenter.commands, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                         VK_PIPELINE_STAGE_TRANSFER_BIT, acquire_transfers);
            cmd_barriers(vk, presenter.commands, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                         VK_PIPELINE_STAGE_TRANSFER_BIT, acquire_layouts);
        } else {
            /* After the bands before, and the layouts the first one set. */
            VkMemoryBarrier after = {};
            after.sType = VK_STRUCTURE_TYPE_MEMORY_BARRIER;
            after.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            after.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT | VK_ACCESS_TRANSFER_WRITE_BIT;
            vk->vkCmdPipelineBarrier(presenter.commands, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                                     VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 1, &after, 0, nullptr, 0,
                                     nullptr);
        }

        if (!use_d3d_staging) {
            VkImageBlit region = {};
            region.srcSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
            region.srcOffsets[0] = {0, top, 0};
            region.srcOffsets[1] = {copy_width, bottom, 1};
            region.dstSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
            region.dstOffsets[0] = {0, top, 0};
            region.dstOffsets[1] = {copy_width, bottom, 1};
            vk->vkCmdBlitImage(presenter.commands, source->image,
                               VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, target.image,
                               VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region, VK_FILTER_NEAREST);
        } else {
            VkImage copy_src = source->image;
            if (!source_is_rgba8) {
                VkImageMemoryBarrier to_dst = image_barrier(
                    target.convert_image, 0, VK_ACCESS_TRANSFER_WRITE_BIT,
                    VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                    VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED);
                vk->vkCmdPipelineBarrier(presenter.commands, VK_PIPELINE_STAGE_TRANSFER_BIT,
                                         VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, nullptr, 0, nullptr,
                                         1, &to_dst);
                VkImageBlit blit = {};
                blit.srcSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
                blit.srcOffsets[0] = {0, top, 0};
                blit.srcOffsets[1] = {copy_width, bottom, 1};
                blit.dstSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
                blit.dstOffsets[0] = {0, top, 0};
                blit.dstOffsets[1] = {copy_width, bottom, 1};
                vk->vkCmdBlitImage(presenter.commands, source->image,
                                   VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, target.convert_image,
                                   VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &blit,
                                   VK_FILTER_NEAREST);
                VkImageMemoryBarrier to_src = image_barrier(
                    target.convert_image, VK_ACCESS_TRANSFER_WRITE_BIT,
                    VK_ACCESS_TRANSFER_READ_BIT, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                    VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_QUEUE_FAMILY_IGNORED,
                    VK_QUEUE_FAMILY_IGNORED);
                vk->vkCmdPipelineBarrier(presenter.commands, VK_PIPELINE_STAGE_TRANSFER_BIT,
                                         VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, nullptr, 0, nullptr,
                                         1, &to_src);
                copy_src = target.convert_image;
            }

            VkBufferImageCopy buf_copy = {};
            buf_copy.bufferOffset = static_cast<VkDeviceSize>(band_y) *
                                    static_cast<VkDeviceSize>(content_width) * 4u;
            buf_copy.bufferRowLength = content_width;
            buf_copy.bufferImageHeight = content_height;
            buf_copy.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
            buf_copy.imageOffset = {0, top, 0};
            buf_copy.imageExtent = {content_width, band_rows, 1};
            vk->vkCmdCopyImageToBuffer(presenter.commands, copy_src,
                                       VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, target.staging_buffer,
                                       1, &buf_copy);

            if (last) {
                VkBufferMemoryBarrier host_vis = {};
                host_vis.sType = VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER;
                host_vis.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
                host_vis.dstAccessMask = VK_ACCESS_HOST_READ_BIT;
                host_vis.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
                host_vis.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
                host_vis.buffer = target.staging_buffer;
                host_vis.offset = 0;
                host_vis.size = static_cast<VkDeviceSize>(content_width) *
                                static_cast<VkDeviceSize>(content_height) * 4u;
                vk->vkCmdPipelineBarrier(presenter.commands, VK_PIPELINE_STAGE_TRANSFER_BIT,
                                         VK_PIPELINE_STAGE_HOST_BIT, 0, 0, nullptr, 1, &host_vis, 0,
                                         nullptr);
            }
        }

        if (last) {
            cmd_barriers(vk, presenter.commands, VK_PIPELINE_STAGE_TRANSFER_BIT,
                         VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, release_layouts);
            cmd_barriers(vk, presenter.commands, VK_PIPELINE_STAGE_TRANSFER_BIT,
                         VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, release_transfers);
        }
        vk->vkEndCommandBuffer(presenter.commands);

        VkSubmitInfo submit = {};
        submit.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
        submit.commandBufferCount = 1;
        submit.pCommandBuffers = &presenter.commands;
        VkResult submitted;
        {
            android::base::AutoLock lock(*emu->getQueueLock());
            submitted = vk->vkQueueSubmit(emu->getQueue(), 1, &submit, presenter.fence);
        }
        if (submitted != VK_SUCCESS) {
            if (submitted == VK_ERROR_DEVICE_LOST) {
                /* The device is made again, and a frame after that finds the new one. */
                emu->noteDeviceLost();
            }
            log_once("submitting the copy of a frame into a task window failed");
            return;
        }
        uint64_t band_ns = 0;
        const dsa::CopyEnd end = finish_copy(*emu, presenter.fence, &band_ns);
        took_ns += band_ns;
        switch (end) {
            case dsa::CopyEnd::Done:
                break;
            case dsa::CopyEnd::DeviceLost:
                ERR("The GPU was reset while a frame of display %u (ColorBuffer %u, %ux%u, frame "
                    "%ux%u) was copied into its task window's %ux%u texture (rows %d to %d, "
                    "band %zu of %zu).",
                    display_id, color_buffer, width, height, content_width, content_height,
                    target.width, target.height, top, bottom, band + 1, bands.size());
                return;
            case dsa::CopyEnd::Hung:
                /* The GPU may still run the command buffer: it and its fence are never used
                 * again, and the next frame makes new ones. */
                ERR("Copying a frame of display %u (ColorBuffer %u, %ux%u, frame %ux%u) into "
                    "its task window's %ux%u texture (rows %d to %d, band %zu of %zu) has not "
                    "finished in %llu ms; the GPU may have hung. The copy is given up and the "
                    "device is checked.",
                    display_id, color_buffer, width, height, content_width, content_height,
                    target.width, target.height, top, bottom, band + 1, bands.size(),
                    static_cast<unsigned long long>(band_ns / 1000000ull));
                presenter.pool = VK_NULL_HANDLE;
                presenter.commands = VK_NULL_HANDLE;
                presenter.fence = VK_NULL_HANDLE;
                return;
        }
    }
    if (use_d3d_staging) {
        if (!target.staging_coherent) {
            VkMappedMemoryRange range = {};
            range.sType = VK_STRUCTURE_TYPE_MAPPED_MEMORY_RANGE;
            range.memory = target.staging_memory;
            range.offset = 0;
            range.size = VK_WHOLE_SIZE;
            vk->vkInvalidateMappedMemoryRanges(emu->getDevice(), 1, &range);
        }
        auto& d3d = presenter.d3d_devices[target.d3d_device_index];
        D3D11_BOX box = {0u, 0u, 0u, content_width, content_height, 1u};
        d3d.context->UpdateSubresource(target.d3d_texture.Get(), 0, &box, target.staging_mapped,
                                       content_width * 4u, 0u);
        d3d.context->End(d3d.event_query.Get());
        d3d.context->Flush();
        const auto wait_started = std::chrono::steady_clock::now();
        BOOL query_done = FALSE;
        while (true) {
            HRESULT hr =
                d3d.context->GetData(d3d.event_query.Get(), &query_done, sizeof(query_done), 0);
            if (hr == S_OK && query_done) {
                break;
            }
            if (FAILED(hr)) {
                log_once("Direct3D 11 shared-texture event query failed");
                return;
            }
            const auto elapsed_ns = static_cast<uint64_t>(
                std::chrono::duration_cast<std::chrono::nanoseconds>(
                    std::chrono::steady_clock::now() - wait_started)
                    .count());
            if (elapsed_ns >= dsa::kCopyWaitNs + dsa::kCopyHungNs) {
                log_once("Direct3D 11 shared-texture upload timed out");
                return;
            }
            std::this_thread::sleep_for(std::chrono::microseconds(250));
        }
    }
    if (took_ns > dsa::kCopySlowNs && slow_copy_may_log()) {
        WARN("Copying a frame of display %u (ColorBuffer %u, %ux%u, frame %ux%u) into its task "
             "window's %ux%u texture took %llu ms.",
             display_id, color_buffer, width, height, content_width, content_height,
             target.width, target.height, static_cast<unsigned long long>(took_ns / 1000000ull));
    }
    presenter.frame_cb(presenter.context, display_id, content_width, content_height);
}

void run(Presenter* presenter) {
    for (;;) {
        std::map<uint32_t, PendingFrame> frames;
        std::vector<uint32_t> forget;
        {
            std::unique_lock<std::mutex> lock(presenter->mutex);
            presenter->wake.wait(lock, [presenter] {
                return presenter->stopping || !presenter->pending.empty() ||
                       !presenter->forget.empty();
            });
            if (presenter->stopping) {
                break;
            }
            frames.swap(presenter->pending);
            forget.swap(presenter->forget);
        }
        if (!forget.empty()) {
            if (VkEmulation* emu = emulation()) {
                auto device_use = emu->useDevice();
                follow_device(*emu, *presenter);
                for (uint32_t display_id : forget) {
                    auto it = presenter->targets.find(display_id);
                    if (it != presenter->targets.end()) {
                        destroy_target(*emu, it->second);
                        presenter->targets.erase(it);
                    }
                }
            }
        }
        for (const auto& [display_id, frame] : frames) {
            present(*presenter, display_id, frame);
        }
    }
    VkEmulation* emu = emulation();
    if (!emu) {
        return;
    }
    auto device_use = emu->useDevice();
    follow_device(*emu, *presenter);
    for (auto& [display_id, target] : presenter->targets) {
        destroy_target(*emu, target);
    }
    presenter->targets.clear();
    VulkanDispatch* vk = emu->getDeviceDispatch();
    if (presenter->fence != VK_NULL_HANDLE) {
        vk->vkDestroyFence(emu->getDevice(), presenter->fence, nullptr);
    }
    if (presenter->pool != VK_NULL_HANDLE) {
        vk->vkDestroyCommandPool(emu->getDevice(), presenter->pool, nullptr);
    }
}

void stop_presenter() {
    Presenter* presenter = nullptr;
    {
        std::lock_guard<std::mutex> lock(s_presenter_mutex);
        presenter = s_presenter;
        s_presenter = nullptr;
    }
    if (!presenter) {
        return;
    }
    {
        std::lock_guard<std::mutex> lock(presenter->mutex);
        presenter->stopping = true;
        presenter->wake.notify_one();
    }
    if (presenter->thread.joinable()) {
        presenter->thread.join();
    }
    delete presenter;
}

}  // namespace

extern "C" DSA_GFX_EXPORT int dsa_gfx_set_display_callbacks2(dsa_display_target_fn target,
                                                             dsa_display_frame_fn frame,
                                                             void* context) {
    stop_presenter();
    if (!target || !frame) {
        return 0;
    }
    if (!emulation()) {
        return -1;
    }
    if (!install_hook()) {
        return -1;
    }
    auto* presenter = new Presenter();
    presenter->target_cb = target;
    presenter->frame_cb = frame;
    presenter->context = context;
    presenter->thread = std::thread(run, presenter);
    std::lock_guard<std::mutex> lock(s_presenter_mutex);
    s_presenter = presenter;
    return 0;
}

extern "C" DSA_GFX_EXPORT void dsa_gfx_forget_display(uint32_t display_id) {
    std::lock_guard<std::mutex> lock(s_presenter_mutex);
    Presenter* presenter = s_presenter;
    if (!presenter) {
        return;
    }
    std::lock_guard<std::mutex> frames(presenter->mutex);
    presenter->pending.erase(display_id);
    presenter->forget.push_back(display_id);
    presenter->wake.notify_one();
}

extern "C" DSA_GFX_EXPORT int dsa_gfx_get_adapter_luid(uint8_t luid[8]) {
    VkEmulation* emu = emulation();
    if (!emu || !luid) {
        return -1;
    }
    VulkanDispatch* ivk = emu->getInstanceDispatch();
    auto get_properties2 = ivk->vkGetPhysicalDeviceProperties2
                               ? ivk->vkGetPhysicalDeviceProperties2
                               : ivk->vkGetPhysicalDeviceProperties2KHR;
    if (!get_properties2) {
        return -1;
    }
    VkPhysicalDeviceIDProperties id_properties = {};
    id_properties.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES;
    VkPhysicalDeviceProperties2 properties = {};
    properties.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2;
    properties.pNext = &id_properties;
    get_properties2(emu->getPhysicalDevice(), &properties);
    if (!id_properties.deviceLUIDValid) {
        return -1;
    }
    std::memcpy(luid, id_properties.deviceLUID, 8);
    return 0;
}

extern "C" DSA_GFX_EXPORT int dsa_gfx_set_display_pixels_callback(dsa_display_pixels_fn,
                                                                  void*) {
    return -1;
}

extern "C" DSA_GFX_EXPORT int dsa_gfx_set_display_dmabuf_callbacks(dsa_display_dmabuf_fn,
                                                                   dsa_display_dmabuf_frame_fn,
                                                                   dsa_display_pixels_fn, void*) {
    return -1;
}

extern "C" DSA_GFX_EXPORT int dsa_gfx_set_display_iosurface_callbacks(
    dsa_display_iosurface_fn, dsa_display_iosurface_frame_fn, dsa_display_pixels_fn, void*) {
    return -1;
}

void dsa_gfx_display_shutdown(void) { stop_presenter(); }

#else /* !_WIN32 */

/* On Linux there is no Direct3D texture to copy into. Each display's newest frame is copied on
 * the GPU into a ring of linear Vulkan images the renderer exports as dmabufs, which the
 * embedder hands to the Wayland compositor (linux-dmabuf): nothing goes through the CPU.
 * Where the driver cannot export a dmabuf, or the embedder asks for it, the frame is read
 * back to memory instead and handed over as pixels. The hook and the pending frames work as
 * on Windows: only the newest frame of each display is kept, so a slow window never makes the
 * guest wait. */

namespace {

using gfxstream::FrameBuffer;
using gfxstream::vk::BorrowedImageInfoVk;
using gfxstream::vk::VkEmulation;
using gfxstream::vk::VulkanDispatch;

constexpr uint32_t kGlRgba = 0x1908;
constexpr uint32_t kGlUnsignedByte = 0x1401;

/* The largest frame shown, in pixels on a side. A size beyond it is a broken guest. */
constexpr uint32_t kMaxSide = 16384;

/* How many images each display cycles through, so the compositor reads one while the next
 * frame is copied into another. */
constexpr uint32_t kSlots = 3;

/* DRM_FORMAT_ABGR8888: R, G, B, A in memory, as VK_FORMAT_R8G8B8A8_UNORM. */
constexpr uint32_t kFourccAbgr8888 = 0x34324241;
constexpr uint64_t kModifierLinear = 0;

struct PendingFrame {
    uint32_t color_buffer = 0;
    uint32_t width = 0;
    uint32_t height = 0;
};

/* One exported image. */
struct Slot {
    uint32_t width = 0;
    uint32_t height = 0;
    VkImage image = VK_NULL_HANDLE;
    VkDeviceMemory memory = VK_NULL_HANDLE;
};

struct Display {
    Slot slots[kSlots];
    uint32_t next = 0;
};

#ifdef __APPLE__
/* CoreFoundation's reference counting, which an IOSurfaceRef follows. */
extern "C" const void* CFRetain(const void* object);
extern "C" void CFRelease(const void* object);

/* VkImportMetalIOSurfaceInfoEXT (VK_EXT_metal_objects), laid out as the Vulkan headers
 * declare it, so the file builds whether or not they were included with the Metal
 * platform. */
struct ImportIoSurfaceInfo {
    VkStructureType sType;
    const void* pNext;
    void* ioSurface;
};
constexpr VkStructureType kImportMetalIoSurfaceInfo = static_cast<VkStructureType>(1000311009);

/* How many of a display's IOSurfaces stay imported: the embedder cycles through two. */
constexpr size_t kImportsPerDisplay = 3;

/* One of the embedder's IOSurfaces, imported into Vulkan and held while it is. */
struct SurfaceImport {
    const void* surface = nullptr;
    uint32_t width = 0;
    uint32_t height = 0;
    VkImage image = VK_NULL_HANDLE;
    VkDeviceMemory memory = VK_NULL_HANDLE;
    uint64_t last_used = 0;
};
#endif

struct PixelPresenter {
    std::mutex mutex;
    std::condition_variable wake;
    std::map<uint32_t, PendingFrame> pending;
    /* Displays the embedder no longer shows. */
    std::vector<uint32_t> forget;
    bool stopping = false;
    std::thread thread;
    dsa_display_pixels_fn pixels_cb = nullptr;
    dsa_display_dmabuf_fn dmabuf_cb = nullptr;
    dsa_display_dmabuf_frame_fn dmabuf_frame_cb = nullptr;
    dsa_display_iosurface_fn iosurface_cb = nullptr;
    dsa_display_iosurface_frame_fn iosurface_frame_cb = nullptr;
    void* context = nullptr;
    /* Only the presentation thread touches these. */
    std::vector<uint8_t> buffer;
    std::map<uint32_t, Display> displays;
    bool dmabuf_failed = false;
    bool iosurface_failed = false;
#ifdef __APPLE__
    std::map<uint32_t, std::vector<SurfaceImport>> imports;
    uint64_t uses = 0;
#endif
    VkCommandPool pool = VK_NULL_HANDLE;
    VkCommandBuffer commands = VK_NULL_HANDLE;
    VkFence fence = VK_NULL_HANDLE;
    /* The renderer's device these objects were made on (VkEmulation::deviceGeneration). */
    uint64_t generation = 0;
};

std::mutex s_presenter_mutex;
PixelPresenter* s_presenter = nullptr;
int (*s_original_set_display_color_buffer)(uint32_t, uint32_t) = nullptr;
int (*s_get_display_pose)(uint32_t, int32_t*, int32_t*, uint32_t*, uint32_t*) = nullptr;
bool s_hook_installed = false;

void log_once(const char* what) {
    static std::mutex log_mutex;
    static std::vector<std::string> said;
    std::lock_guard<std::mutex> lock(log_mutex);
    for (const auto& line : said) {
        if (line == what) {
            return;
        }
    }
    said.emplace_back(what);
    std::fprintf(stderr, "dsa_gfx: %s\n", what);
}

VkEmulation* emulation() {
    FrameBuffer* fb = FrameBuffer::getFB();
    if (!fb || !fb->hasEmulationVk()) {
        return nullptr;
    }
    return &fb->getEmulationVk();
}

int hooked_set_display_color_buffer(uint32_t display_id, uint32_t color_buffer) {
    int result = s_original_set_display_color_buffer
                     ? s_original_set_display_color_buffer(display_id, color_buffer)
                     : 0;
    if (color_buffer == 0) {
        return result;
    }
    PendingFrame frame;
    frame.color_buffer = color_buffer;
    if (s_get_display_pose) {
        int32_t x = 0;
        int32_t y = 0;
        uint32_t w = 0;
        uint32_t h = 0;
        if (s_get_display_pose(display_id, &x, &y, &w, &h) == 0) {
            frame.width = w;
            frame.height = h;
        }
    }
    std::lock_guard<std::mutex> lock(s_presenter_mutex);
    if (s_presenter) {
        std::lock_guard<std::mutex> pending_lock(s_presenter->mutex);
        s_presenter->pending[display_id] = frame;
        s_presenter->wake.notify_one();
    }
    return result;
}

bool install_hook() {
    if (s_hook_installed) {
        return true;
    }
    QAndroidMultiDisplayAgent operations = emugl::get_emugl_multi_display_operations();
    s_original_set_display_color_buffer = operations.setDisplayColorBuffer;
    s_get_display_pose = operations.getDisplayPose;
    operations.setDisplayColorBuffer = hooked_set_display_color_buffer;
    emugl::set_emugl_multi_display_operations(operations);
    s_hook_installed = true;
    return true;
}

void destroy_slot(VkEmulation& emu, Slot& slot) {
    VulkanDispatch* vk = emu.getDeviceDispatch();
    if (slot.image != VK_NULL_HANDLE) {
        vk->vkDestroyImage(emu.getDevice(), slot.image, nullptr);
    }
    if (slot.memory != VK_NULL_HANDLE) {
        vk->vkFreeMemory(emu.getDevice(), slot.memory, nullptr);
    }
    slot = Slot{};
}

/* Makes a linear image the copy can write and the compositor can read, and exports its
 * memory as a dmabuf to the embedder. */
bool export_slot(VkEmulation& emu, PixelPresenter& presenter, uint32_t display_id,
                 uint32_t index, uint32_t width, uint32_t height, Slot* out) {
    VulkanDispatch* vk = emu.getDeviceDispatch();
    VkDevice device = emu.getDevice();
    if (!emu.supportsDmaBuf()) {
        log_once("the Vulkan device cannot export dmabufs (VK_EXT_external_memory_dma_buf); "
                 "frames are read back instead");
        return false;
    }
    if (!vk->vkGetMemoryFdKHR) {
        log_once("the Vulkan driver cannot export memory (VK_KHR_external_memory_fd); "
                 "frames are read back instead");
        return false;
    }
    VkExternalMemoryImageCreateInfo external_info = {};
    external_info.sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO;
    external_info.handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT;

    VkImageCreateInfo image_info = {};
    image_info.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
    image_info.pNext = &external_info;
    image_info.imageType = VK_IMAGE_TYPE_2D;
    image_info.format = VK_FORMAT_R8G8B8A8_UNORM;
    image_info.extent = {width, height, 1};
    image_info.mipLevels = 1;
    image_info.arrayLayers = 1;
    image_info.samples = VK_SAMPLE_COUNT_1_BIT;
    image_info.tiling = VK_IMAGE_TILING_LINEAR;
    image_info.usage = VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    image_info.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    image_info.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;

    Slot slot;
    slot.width = width;
    slot.height = height;
    if (vk->vkCreateImage(device, &image_info, nullptr, &slot.image) != VK_SUCCESS) {
        log_once("the Vulkan driver cannot make an exportable image; frames are read back");
        return false;
    }
    VkMemoryRequirements requirements = {};
    vk->vkGetImageMemoryRequirements(device, slot.image, &requirements);
    uint32_t type_index = UINT32_MAX;
    VkPhysicalDeviceMemoryProperties memory = {};
    emu.getInstanceDispatch()->vkGetPhysicalDeviceMemoryProperties(emu.getPhysicalDevice(),
                                                                   &memory);
    for (uint32_t i = 0; i < memory.memoryTypeCount; i++) {
        if ((requirements.memoryTypeBits & (1u << i)) &&
            (memory.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)) {
            type_index = i;
            break;
        }
    }
    if (type_index == UINT32_MAX) {
        for (uint32_t i = 0; i < memory.memoryTypeCount; i++) {
            if (requirements.memoryTypeBits & (1u << i)) {
                type_index = i;
                break;
            }
        }
    }
    if (type_index == UINT32_MAX) {
        destroy_slot(emu, slot);
        return false;
    }
    VkMemoryDedicatedAllocateInfo dedicated = {};
    dedicated.sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO;
    dedicated.image = slot.image;
    VkExportMemoryAllocateInfo export_info = {};
    export_info.sType = VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO;
    export_info.pNext = &dedicated;
    export_info.handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT;
    VkMemoryAllocateInfo allocate_info = {};
    allocate_info.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    allocate_info.pNext = &export_info;
    allocate_info.allocationSize = requirements.size;
    allocate_info.memoryTypeIndex = type_index;
    if (vk->vkAllocateMemory(device, &allocate_info, nullptr, &slot.memory) != VK_SUCCESS ||
        vk->vkBindImageMemory(device, slot.image, slot.memory, 0) != VK_SUCCESS) {
        log_once("the Vulkan driver cannot export an image as a dmabuf; frames are read back");
        destroy_slot(emu, slot);
        return false;
    }
    VkMemoryGetFdInfoKHR fd_info = {};
    fd_info.sType = VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR;
    fd_info.memory = slot.memory;
    fd_info.handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT;
    int fd = -1;
    if (vk->vkGetMemoryFdKHR(device, &fd_info, &fd) != VK_SUCCESS || fd < 0) {
        log_once("the Vulkan driver did not export a dmabuf; frames are read back");
        destroy_slot(emu, slot);
        return false;
    }
    VkImageSubresource subresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0};
    VkSubresourceLayout layout = {};
    vk->vkGetImageSubresourceLayout(device, slot.image, &subresource, &layout);
    /* The embedder owns the descriptor from here, whatever it answers. */
    if (presenter.dmabuf_cb(presenter.context, display_id, index, width, height, fd,
                            static_cast<uint32_t>(layout.rowPitch),
                            static_cast<uint32_t>(layout.offset), kFourccAbgr8888,
                            kModifierLinear) != 0) {
        destroy_slot(emu, slot);
        return false;
    }
    *out = slot;
    return true;
}

bool ensure_commands(VkEmulation& emu, PixelPresenter& presenter) {
    if (presenter.commands != VK_NULL_HANDLE) {
        return true;
    }
    VulkanDispatch* vk = emu.getDeviceDispatch();
    VkDevice device = emu.getDevice();
    VkCommandPoolCreateInfo pool_info = {};
    pool_info.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO;
    pool_info.flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    pool_info.queueFamilyIndex = emu.getQueueFamilyIndex();
    if (vk->vkCreateCommandPool(device, &pool_info, nullptr, &presenter.pool) != VK_SUCCESS) {
        return false;
    }
    VkCommandBufferAllocateInfo buffer_info = {};
    buffer_info.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
    buffer_info.commandPool = presenter.pool;
    buffer_info.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    buffer_info.commandBufferCount = 1;
    if (vk->vkAllocateCommandBuffers(device, &buffer_info, &presenter.commands) != VK_SUCCESS) {
        return false;
    }
    VkFenceCreateInfo fence_info = {};
    fence_info.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO;
    return vk->vkCreateFence(device, &fence_info, nullptr, &presenter.fence) == VK_SUCCESS;
}

VkImageMemoryBarrier image_barrier(VkImage image, VkAccessFlags src_access,
                                   VkAccessFlags dst_access, VkImageLayout old_layout,
                                   VkImageLayout new_layout, uint32_t src_family,
                                   uint32_t dst_family) {
    VkImageMemoryBarrier barrier = {};
    barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
    barrier.srcAccessMask = src_access;
    barrier.dstAccessMask = dst_access;
    barrier.oldLayout = old_layout;
    barrier.newLayout = new_layout;
    barrier.srcQueueFamilyIndex = src_family;
    barrier.dstQueueFamilyIndex = dst_family;
    barrier.image = image;
    barrier.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    return barrier;
}

void cmd_barriers(VulkanDispatch* vk, VkCommandBuffer commands, VkPipelineStageFlags src,
                  VkPipelineStageFlags dst, const std::vector<VkImageMemoryBarrier>& barriers) {
    if (barriers.empty()) {
        return;
    }
    vk->vkCmdPipelineBarrier(commands, src, dst, 0, 0, nullptr, 0, nullptr,
                             static_cast<uint32_t>(barriers.size()), barriers.data());
}

/* The barriers a copy into the embedder's image takes: the first band takes the images over
 * (`acquire_*`), the last hands them back (`release_*`). */
struct CopyBarriers {
    std::vector<VkImageMemoryBarrier> acquire_transfers;
    std::vector<VkImageMemoryBarrier> acquire_layouts;
    std::vector<VkImageMemoryBarrier> release_layouts;
    std::vector<VkImageMemoryBarrier> release_transfers;
};

/* Copies the top-left `width` x `height` of `source` into `target` on the GPU. The copy goes in
 * bands of at most dsa::kMaxCopyPixels, each a submission of its own that is waited for before
 * the next, as on Windows: a frame of a window maximized on a HiDPI monitor (2880x1800 and more)
 * is never one GPU job, and the GPU can turn to other work between the bands. A blit (`blit`)
 * converts between formats, as from the ColorBuffer's RGBA into a BGRA IOSurface; otherwise the
 * pixels are copied as they are. True when the whole frame was copied; false when the copy was
 * given up (the device was lost or hung, or the submission failed), and the frame is dropped. */
bool copy_in_bands(VkEmulation& emu, PixelPresenter& presenter, uint32_t display_id,
                   VkImage source, VkImage target, uint32_t width, uint32_t height, bool blit,
                   const CopyBarriers& barriers) {
    VulkanDispatch* vk = emu.getDeviceDispatch();
    const std::vector<dsa::CopyBand> bands = dsa::copy_bands(width, height);
    uint64_t took_ns = 0;
    for (size_t band = 0; band < bands.size(); ++band) {
        const bool first = band == 0;
        const bool last = band + 1 == bands.size();
        const int32_t top = static_cast<int32_t>(bands[band].y);
        const uint32_t rows = bands[band].rows;

        VkCommandBufferBeginInfo begin = {};
        begin.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
        begin.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
        vk->vkResetCommandBuffer(presenter.commands, 0);
        vk->vkBeginCommandBuffer(presenter.commands, &begin);
        if (first) {
            cmd_barriers(vk, presenter.commands, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                         VK_PIPELINE_STAGE_TRANSFER_BIT, barriers.acquire_transfers);
            cmd_barriers(vk, presenter.commands, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                         VK_PIPELINE_STAGE_TRANSFER_BIT, barriers.acquire_layouts);
        } else {
            /* After the bands before, and the layouts the first one set. */
            VkMemoryBarrier after = {};
            after.sType = VK_STRUCTURE_TYPE_MEMORY_BARRIER;
            after.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
            after.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT | VK_ACCESS_TRANSFER_WRITE_BIT;
            vk->vkCmdPipelineBarrier(presenter.commands, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                                     VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 1, &after, 0, nullptr, 0,
                                     nullptr);
        }
        if (blit) {
            /* The sizes match, so nothing is scaled. */
            VkImageBlit region = {};
            region.srcSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
            region.srcOffsets[0] = {0, top, 0};
            region.srcOffsets[1] = {static_cast<int32_t>(width),
                                    top + static_cast<int32_t>(rows), 1};
            region.dstSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
            region.dstOffsets[0] = region.srcOffsets[0];
            region.dstOffsets[1] = region.srcOffsets[1];
            vk->vkCmdBlitImage(presenter.commands, source, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                               target, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region,
                               VK_FILTER_NEAREST);
        } else {
            VkImageCopy region = {};
            region.srcSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
            region.srcOffset = {0, top, 0};
            region.dstSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1};
            region.dstOffset = {0, top, 0};
            region.extent = {width, rows, 1};
            vk->vkCmdCopyImage(presenter.commands, source, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                               target, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region);
        }
        if (last) {
            cmd_barriers(vk, presenter.commands, VK_PIPELINE_STAGE_TRANSFER_BIT,
                         VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, barriers.release_layouts);
            cmd_barriers(vk, presenter.commands, VK_PIPELINE_STAGE_TRANSFER_BIT,
                         VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, barriers.release_transfers);
        }
        vk->vkEndCommandBuffer(presenter.commands);

        VkSubmitInfo submit = {};
        submit.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO;
        submit.commandBufferCount = 1;
        submit.pCommandBuffers = &presenter.commands;
        VkResult submitted;
        {
            android::base::AutoLock lock(*emu.getQueueLock());
            submitted = vk->vkQueueSubmit(emu.getQueue(), 1, &submit, presenter.fence);
        }
        if (submitted != VK_SUCCESS) {
            if (submitted == VK_ERROR_DEVICE_LOST) {
                /* The device is made again, and a frame after that finds the new one. */
                emu.noteDeviceLost();
            }
            log_once("submitting the copy of a frame into a task window failed");
            return false;
        }
        uint64_t band_ns = 0;
        const dsa::CopyEnd end = finish_copy(emu, presenter.fence, &band_ns);
        took_ns += band_ns;
        switch (end) {
            case dsa::CopyEnd::Done:
                break;
            case dsa::CopyEnd::DeviceLost:
                ERR("The GPU was reset while a frame of display %u (%ux%u) was copied into its "
                    "task window (rows %d to %d, band %zu of %zu).",
                    display_id, width, height, top, top + static_cast<int32_t>(rows), band + 1,
                    bands.size());
                return false;
            case dsa::CopyEnd::Hung:
                /* The GPU may still run the command buffer: it and its fence are never used
                 * again, and the next frame makes new ones. */
                ERR("Copying a frame of display %u (%ux%u) into its task window (rows %d to %d, "
                    "band %zu of %zu) has not finished in %llu ms; the GPU may have hung. The "
                    "copy is given up and the device is checked.",
                    display_id, width, height, top, top + static_cast<int32_t>(rows), band + 1,
                    bands.size(), static_cast<unsigned long long>(band_ns / 1000000ull));
                presenter.pool = VK_NULL_HANDLE;
                presenter.commands = VK_NULL_HANDLE;
                presenter.fence = VK_NULL_HANDLE;
                return false;
        }
    }
    if (took_ns > dsa::kCopySlowNs && slow_copy_may_log()) {
        WARN("Copying a frame of display %u (%ux%u, in %zu band(s)) into its task window took "
             "%llu ms.",
             display_id, width, height, bands.size(),
             static_cast<unsigned long long>(took_ns / 1000000ull));
    }
    return true;
}

/* Copies the frame into the display's next dmabuf on the GPU. False when it cannot, and the
 * frame is read back instead. */
bool present_dmabuf(PixelPresenter& presenter, uint32_t display_id, const PendingFrame& frame) {
    VkEmulation* emu = emulation();
    FrameBuffer* fb = FrameBuffer::getFB();
    if (!emu || !fb) {
        return false;
    }
    auto keep_alive = fb->findColorBuffer(frame.color_buffer);
    if (!keep_alive) {
        return true;
    }
    const uint32_t width = keep_alive->getWidth();
    const uint32_t height = keep_alive->getHeight();
    if (width == 0 || height == 0 || width > kMaxSide || height > kMaxSide) {
        return true;
    }
    Display& display = presenter.displays[display_id];
    const uint32_t index = display.next;
    Slot& slot = display.slots[index];
    if (slot.width != width || slot.height != height) {
        destroy_slot(*emu, slot);
        if (!export_slot(*emu, presenter, display_id, index, width, height, &slot)) {
            return false;
        }
    }
    if (!ensure_commands(*emu, presenter)) {
        log_once("could not set up the Vulkan commands that copy frames into task windows");
        return false;
    }
    std::unique_ptr<BorrowedImageInfoVk> source = emu->borrowColorBufferForDisplay(frame.color_buffer);
    if (!source) {
        return true;
    }
    uint32_t content_width = frame.width ? frame.width : width;
    uint32_t content_height = frame.height ? frame.height : height;
    content_width = std::min(std::min(content_width, width), source->imageCreateInfo.extent.width);
    content_height =
        std::min(std::min(content_height, height), source->imageCreateInfo.extent.height);
    if (content_width == 0 || content_height == 0) {
        return true;
    }
    const uint32_t family = emu->getQueueFamilyIndex();

    CopyBarriers barriers;
    gfxstream::vk::addNeededBarriersToUseBorrowedImage(
        *source, family, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_ACCESS_TRANSFER_READ_BIT,
        &barriers.acquire_transfers, &barriers.acquire_layouts, &barriers.release_layouts,
        &barriers.release_transfers);
    barriers.acquire_layouts.push_back(image_barrier(
        slot.image, 0, VK_ACCESS_TRANSFER_WRITE_BIT, VK_IMAGE_LAYOUT_UNDEFINED,
        VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED));
    barriers.release_transfers.push_back(image_barrier(
        slot.image, VK_ACCESS_TRANSFER_WRITE_BIT, 0, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
        VK_IMAGE_LAYOUT_GENERAL, family, VK_QUEUE_FAMILY_FOREIGN_EXT));
    /* In bands, none of them one GPU job the size of the window. */
    if (!copy_in_bands(*emu, presenter, display_id, source->image, slot.image, content_width,
                       content_height, false, barriers)) {
        return true;
    }
    display.next = (index + 1) % kSlots;
    presenter.dmabuf_frame_cb(presenter.context, display_id, index, content_width,
                              content_height);
    return true;
}

#ifdef __APPLE__

void release_import(VkEmulation& emu, SurfaceImport& import) {
    VulkanDispatch* vk = emu.getDeviceDispatch();
    if (import.image != VK_NULL_HANDLE) {
        vk->vkDestroyImage(emu.getDevice(), import.image, nullptr);
    }
    if (import.memory != VK_NULL_HANDLE) {
        vk->vkFreeMemory(emu.getDevice(), import.memory, nullptr);
    }
    if (import.surface) {
        CFRelease(import.surface);
    }
    import = SurfaceImport{};
}

void forget_display(PixelPresenter& presenter, uint32_t display_id) {
    auto found = presenter.imports.find(display_id);
    if (found == presenter.imports.end()) {
        return;
    }
    VkEmulation* emu = emulation();
    if (emu) {
        for (SurfaceImport& import : found->second) {
            release_import(*emu, import);
        }
    }
    presenter.imports.erase(found);
}

/* Whether the Vulkan device can take an IOSurface as an image's storage. */
bool imports_iosurfaces(VkEmulation& emu) {
    static int answer = -1;
    if (answer >= 0) {
        return answer == 1;
    }
    answer = 0;
    VulkanDispatch* ivk = emu.getInstanceDispatch();
    uint32_t count = 0;
    if (ivk->vkEnumerateDeviceExtensionProperties(emu.getPhysicalDevice(), nullptr, &count,
                                                  nullptr) != VK_SUCCESS) {
        return false;
    }
    std::vector<VkExtensionProperties> extensions(count);
    if (ivk->vkEnumerateDeviceExtensionProperties(emu.getPhysicalDevice(), nullptr, &count,
                                                  extensions.data()) != VK_SUCCESS) {
        return false;
    }
    for (const VkExtensionProperties& extension : extensions) {
        if (std::strcmp(extension.extensionName, "VK_EXT_metal_objects") == 0) {
            answer = 1;
        }
    }
    return answer == 1;
}

/* Makes a Vulkan image whose storage is the embedder's IOSurface, and holds the surface. */
bool import_surface(VkEmulation& emu, const void* surface, uint32_t width, uint32_t height,
                    SurfaceImport* out) {
    VulkanDispatch* vk = emu.getDeviceDispatch();
    VkDevice device = emu.getDevice();
    ImportIoSurfaceInfo import_info = {};
    import_info.sType = kImportMetalIoSurfaceInfo;
    import_info.ioSurface = const_cast<void*>(surface);

    VkImageCreateInfo image_info = {};
    image_info.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
    image_info.pNext = &import_info;
    image_info.imageType = VK_IMAGE_TYPE_2D;
    image_info.format = VK_FORMAT_B8G8R8A8_UNORM;
    image_info.extent = {width, height, 1};
    image_info.mipLevels = 1;
    image_info.arrayLayers = 1;
    image_info.samples = VK_SAMPLE_COUNT_1_BIT;
    image_info.tiling = VK_IMAGE_TILING_OPTIMAL;
    image_info.usage = VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    image_info.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
    image_info.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;

    SurfaceImport import;
    import.width = width;
    import.height = height;
    if (vk->vkCreateImage(device, &image_info, nullptr, &import.image) != VK_SUCCESS) {
        log_once("the Vulkan driver cannot make an image of an IOSurface; frames are read back");
        return false;
    }
    VkMemoryRequirements requirements = {};
    vk->vkGetImageMemoryRequirements(device, import.image, &requirements);
    VkPhysicalDeviceMemoryProperties memory = {};
    emu.getInstanceDispatch()->vkGetPhysicalDeviceMemoryProperties(emu.getPhysicalDevice(),
                                                                   &memory);
    uint32_t type_index = UINT32_MAX;
    for (uint32_t i = 0; i < memory.memoryTypeCount; i++) {
        if ((requirements.memoryTypeBits & (1u << i)) &&
            (memory.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)) {
            type_index = i;
            break;
        }
    }
    if (type_index == UINT32_MAX) {
        for (uint32_t i = 0; i < memory.memoryTypeCount; i++) {
            if (requirements.memoryTypeBits & (1u << i)) {
                type_index = i;
                break;
            }
        }
    }
    /* MoltenVK keeps the image's texture on the IOSurface; the memory bound is only what
     * the API asks for. */
    VkMemoryDedicatedAllocateInfo dedicated = {};
    dedicated.sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO;
    dedicated.image = import.image;
    VkMemoryAllocateInfo allocate_info = {};
    allocate_info.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    allocate_info.pNext = &dedicated;
    allocate_info.allocationSize = requirements.size;
    allocate_info.memoryTypeIndex = type_index;
    if (type_index == UINT32_MAX ||
        vk->vkAllocateMemory(device, &allocate_info, nullptr, &import.memory) != VK_SUCCESS ||
        vk->vkBindImageMemory(device, import.image, import.memory, 0) != VK_SUCCESS) {
        log_once("the Vulkan driver cannot bind an IOSurface image; frames are read back");
        release_import(emu, import);
        return false;
    }
    import.surface = CFRetain(surface);
    *out = import;
    return true;
}

/* The display's import of `surface`, made when it is new. The oldest import goes when the
 * display holds more than it cycles through. */
SurfaceImport* surface_import(VkEmulation& emu, PixelPresenter& presenter, uint32_t display_id,
                              const void* surface, uint32_t width, uint32_t height) {
    std::vector<SurfaceImport>& imports = presenter.imports[display_id];
    presenter.uses++;
    for (SurfaceImport& import : imports) {
        if (import.surface == surface && import.width == width && import.height == height) {
            import.last_used = presenter.uses;
            return &import;
        }
    }
    SurfaceImport import;
    if (!import_surface(emu, surface, width, height, &import)) {
        return nullptr;
    }
    import.last_used = presenter.uses;
    while (imports.size() >= kImportsPerDisplay) {
        auto oldest = std::min_element(imports.begin(), imports.end(),
                                       [](const SurfaceImport& a, const SurfaceImport& b) {
                                           return a.last_used < b.last_used;
                                       });
        release_import(emu, *oldest);
        imports.erase(oldest);
    }
    imports.push_back(import);
    return &imports.back();
}

/* Copies the frame into the IOSurface the embedder gives, on the GPU. False when the device
 * cannot, and the frame is read back instead. */
bool present_iosurface(PixelPresenter& presenter, uint32_t display_id, const PendingFrame& frame) {
    VkEmulation* emu = emulation();
    FrameBuffer* fb = FrameBuffer::getFB();
    if (!emu || !fb) {
        return false;
    }
    if (!imports_iosurfaces(*emu)) {
        log_once("the Vulkan device has no VK_EXT_metal_objects; frames are read back");
        return false;
    }
    auto keep_alive = fb->findColorBuffer(frame.color_buffer);
    if (!keep_alive) {
        return true;
    }
    const uint32_t width = keep_alive->getWidth();
    const uint32_t height = keep_alive->getHeight();
    if (width == 0 || height == 0 || width > kMaxSide || height > kMaxSide) {
        return true;
    }
    void* surface = presenter.iosurface_cb(presenter.context, display_id, width, height);
    if (!surface) {
        /* Shown nowhere: what was imported for the display goes. */
        forget_display(presenter, display_id);
        return true;
    }
    SurfaceImport* target = surface_import(*emu, presenter, display_id, surface, width, height);
    if (!target) {
        return false;
    }
    if (!ensure_commands(*emu, presenter)) {
        log_once("could not set up the Vulkan commands that copy frames into task windows");
        return false;
    }
    std::unique_ptr<BorrowedImageInfoVk> source = emu->borrowColorBufferForDisplay(frame.color_buffer);
    if (!source) {
        return true;
    }
    uint32_t content_width = frame.width ? frame.width : width;
    uint32_t content_height = frame.height ? frame.height : height;
    content_width = std::min(std::min(content_width, width), source->imageCreateInfo.extent.width);
    content_height =
        std::min(std::min(content_height, height), source->imageCreateInfo.extent.height);
    if (content_width == 0 || content_height == 0) {
        return true;
    }
    const uint32_t family = emu->getQueueFamilyIndex();

    CopyBarriers barriers;
    gfxstream::vk::addNeededBarriersToUseBorrowedImage(
        *source, family, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_ACCESS_TRANSFER_READ_BIT,
        &barriers.acquire_transfers, &barriers.acquire_layouts, &barriers.release_layouts,
        &barriers.release_transfers);
    barriers.acquire_layouts.push_back(image_barrier(
        target->image, 0, VK_ACCESS_TRANSFER_WRITE_BIT, VK_IMAGE_LAYOUT_UNDEFINED,
        VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED));
    barriers.release_layouts.push_back(image_barrier(
        target->image, VK_ACCESS_TRANSFER_WRITE_BIT, 0, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
        VK_IMAGE_LAYOUT_GENERAL, VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED));
    /* A blit, not a copy: the color buffer is RGBA and the surface BGRA, and only a blit
     * converts between them. In bands, none of them one GPU job the size of the window. */
    if (!copy_in_bands(*emu, presenter, display_id, source->image, target->image, content_width,
                       content_height, true, barriers)) {
        return true;
    }
    presenter.iosurface_frame_cb(presenter.context, display_id, content_width, content_height);
    return true;
}

#endif /* __APPLE__ */

/* Reads the frame back to memory. The frame fills the top-left corner of a ColorBuffer the
 * size of the display, and Vulkan reads back only a whole ColorBuffer (a part of one fails
 * with "unhandled subrect" and leaves nothing), so the whole buffer is read and the frame's
 * rows are kept from it (dsa_frame_crop.h). */
void present_pixels(PixelPresenter& presenter, uint32_t display_id, const PendingFrame& frame) {
    FrameBuffer* fb = FrameBuffer::getFB();
    if (!fb || !presenter.pixels_cb) {
        return;
    }
    auto keep_alive = fb->findColorBuffer(frame.color_buffer);
    if (!keep_alive) {
        return;
    }
    const uint32_t buffer_width = keep_alive->getWidth();
    const uint32_t buffer_height = keep_alive->getHeight();
    if (buffer_width == 0 || buffer_height == 0 || buffer_width > kMaxSide ||
        buffer_height > kMaxSide) {
        return;
    }
    const dsa::FrameSize shown =
        dsa::frame_in_buffer(buffer_width, buffer_height, frame.width, frame.height);
    const uint64_t size = uint64_t(buffer_width) * buffer_height * 4;
    presenter.buffer.resize(size);
    fb->readColorBuffer(frame.color_buffer, 0, 0, int(buffer_width), int(buffer_height),
                        kGlRgba, kGlUnsignedByte, presenter.buffer.data(), size);
    dsa::keep_top_left(presenter.buffer.data(), buffer_width, shown.width, shown.height);
    presenter.pixels_cb(presenter.context, display_id, shown.width, shown.height,
                        presenter.buffer.data());
}

/* The renderer made a new device after a GPU reset: what was made on the old one went with
 * it, so it is forgotten, not destroyed, and made again as frames come. */
void follow_device(VkEmulation& emu, PixelPresenter& presenter) {
    /* A device being made again may be gone already: nothing is destroyed on it. */
    const uint64_t generation = emu.deviceUsable() ? emu.deviceGeneration() : kNoDevice;
    if (presenter.generation == generation) {
        return;
    }
    presenter.generation = generation;
    presenter.displays.clear();
#ifdef __APPLE__
    /* The surfaces stay the embedder's; only this side's hold on them goes. */
    for (auto& [display_id, imports] : presenter.imports) {
        for (SurfaceImport& import : imports) {
            if (import.surface) {
                CFRelease(import.surface);
            }
        }
    }
    presenter.imports.clear();
#endif
    presenter.dmabuf_failed = false;
    presenter.iosurface_failed = false;
    presenter.pool = VK_NULL_HANDLE;
    presenter.commands = VK_NULL_HANDLE;
    presenter.fence = VK_NULL_HANDLE;
    log_once("the renderer made a new Vulkan device after a GPU reset; task windows follow it");
}

void present(PixelPresenter& presenter, uint32_t display_id, const PendingFrame& frame) {
    if (VkEmulation* emu = emulation()) {
        /* A lost device is made again before the copy, and none is made while it is used. */
        if (!emu->recoverIfLost()) {
            return;
        }
        auto device_use = emu->useDevice();
        follow_device(*emu, presenter);
#ifdef __APPLE__
        if (presenter.iosurface_cb && presenter.iosurface_frame_cb && !presenter.iosurface_failed) {
            if (present_iosurface(presenter, display_id, frame)) {
                return;
            }
            presenter.iosurface_failed = true;
        }
#endif
        if (presenter.dmabuf_cb && presenter.dmabuf_frame_cb && !presenter.dmabuf_failed) {
            if (present_dmabuf(presenter, display_id, frame)) {
                return;
            }
            presenter.dmabuf_failed = true;
        }
    }
    /* Read back without the device lock: the read makes a lost device again itself. */
    present_pixels(presenter, display_id, frame);
}

void run(PixelPresenter* presenter) {
    for (;;) {
        std::map<uint32_t, PendingFrame> frames;
        std::vector<uint32_t> forget;
        {
            std::unique_lock<std::mutex> lock(presenter->mutex);
            presenter->wake.wait(lock, [presenter] {
                return presenter->stopping || !presenter->pending.empty() ||
                       !presenter->forget.empty();
            });
            if (presenter->stopping) {
                break;
            }
            frames.swap(presenter->pending);
            forget.swap(presenter->forget);
        }
#ifdef __APPLE__
        if (VkEmulation* emu = emulation()) {
            auto device_use = emu->useDevice();
            follow_device(*emu, *presenter);
            for (uint32_t display_id : forget) {
                forget_display(*presenter, display_id);
            }
        }
#endif
        for (const auto& [display_id, frame] : frames) {
            present(*presenter, display_id, frame);
        }
    }
    VkEmulation* emu = emulation();
    if (!emu) {
        return;
    }
    auto device_use = emu->useDevice();
    follow_device(*emu, *presenter);
#ifdef __APPLE__
    for (auto& [display_id, imports] : presenter->imports) {
        for (SurfaceImport& import : imports) {
            release_import(*emu, import);
        }
    }
    presenter->imports.clear();
#endif
    for (auto& [display_id, display] : presenter->displays) {
        for (Slot& slot : display.slots) {
            destroy_slot(*emu, slot);
        }
    }
    VulkanDispatch* vk = emu->getDeviceDispatch();
    if (presenter->fence != VK_NULL_HANDLE) {
        vk->vkDestroyFence(emu->getDevice(), presenter->fence, nullptr);
    }
    if (presenter->pool != VK_NULL_HANDLE) {
        vk->vkDestroyCommandPool(emu->getDevice(), presenter->pool, nullptr);
    }
}

void stop_presenter() {
    PixelPresenter* presenter = nullptr;
    {
        std::lock_guard<std::mutex> lock(s_presenter_mutex);
        presenter = s_presenter;
        s_presenter = nullptr;
    }
    if (!presenter) {
        return;
    }
    {
        std::lock_guard<std::mutex> lock(presenter->mutex);
        presenter->stopping = true;
        presenter->wake.notify_one();
    }
    if (presenter->thread.joinable()) {
        presenter->thread.join();
    }
    delete presenter;
}

/* The ways a presentation can hand frames over; the first that works for a frame is used. */
struct Handover {
    dsa_display_pixels_fn pixels = nullptr;
    dsa_display_dmabuf_fn dmabuf = nullptr;
    dsa_display_dmabuf_frame_fn dmabuf_frame = nullptr;
    dsa_display_iosurface_fn iosurface = nullptr;
    dsa_display_iosurface_frame_fn iosurface_frame = nullptr;
};

int start_presenter(const Handover& handover, void* context) {
    stop_presenter();
    if (!handover.pixels && !(handover.dmabuf && handover.dmabuf_frame) &&
        !(handover.iosurface && handover.iosurface_frame)) {
        return 0;
    }
    if (!FrameBuffer::getFB()) {
        return -1;
    }
    if (!install_hook()) {
        return -1;
    }
    auto* presenter = new PixelPresenter();
    presenter->pixels_cb = handover.pixels;
    presenter->dmabuf_cb = handover.dmabuf;
    presenter->dmabuf_frame_cb = handover.dmabuf_frame;
    presenter->iosurface_cb = handover.iosurface;
    presenter->iosurface_frame_cb = handover.iosurface_frame;
    presenter->context = context;
    presenter->thread = std::thread(run, presenter);
    std::lock_guard<std::mutex> lock(s_presenter_mutex);
    s_presenter = presenter;
    return 0;
}

}  // namespace

extern "C" DSA_GFX_EXPORT int dsa_gfx_set_display_callbacks2(dsa_display_target_fn,
                                                             dsa_display_frame_fn, void*) {
    return -1;
}

extern "C" DSA_GFX_EXPORT int dsa_gfx_set_display_pixels_callback(dsa_display_pixels_fn pixels,
                                                                  void* context) {
    Handover handover;
    handover.pixels = pixels;
    return start_presenter(handover, context);
}

extern "C" DSA_GFX_EXPORT int dsa_gfx_set_display_dmabuf_callbacks(
    dsa_display_dmabuf_fn dmabuf, dsa_display_dmabuf_frame_fn frame,
    dsa_display_pixels_fn fallback, void* context) {
    if (dmabuf && frame && !emulation()) {
        return -1;
    }
    Handover handover;
    handover.pixels = fallback;
    handover.dmabuf = dmabuf;
    handover.dmabuf_frame = frame;
    return start_presenter(handover, context);
}

extern "C" DSA_GFX_EXPORT int dsa_gfx_set_display_iosurface_callbacks(
    dsa_display_iosurface_fn target, dsa_display_iosurface_frame_fn frame,
    dsa_display_pixels_fn fallback, void* context) {
#ifdef __APPLE__
    if (target && frame && !emulation()) {
        return -1;
    }
    Handover handover;
    handover.pixels = fallback;
    handover.iosurface = target;
    handover.iosurface_frame = frame;
    return start_presenter(handover, context);
#else
    (void)target;
    (void)frame;
    (void)fallback;
    (void)context;
    return -1;
#endif
}

extern "C" DSA_GFX_EXPORT void dsa_gfx_forget_display(uint32_t display_id) {
    std::lock_guard<std::mutex> lock(s_presenter_mutex);
    if (s_presenter) {
        std::lock_guard<std::mutex> pending_lock(s_presenter->mutex);
        s_presenter->pending.erase(display_id);
        s_presenter->forget.push_back(display_id);
        s_presenter->wake.notify_one();
    }
}

extern "C" DSA_GFX_EXPORT int dsa_gfx_get_adapter_luid(uint8_t*) { return -1; }

void dsa_gfx_display_shutdown(void) { stop_presenter(); }

#endif /* _WIN32 */
