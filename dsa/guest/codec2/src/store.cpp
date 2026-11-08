// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "store.h"

#include <chrono>
#include <cstring>

#include <C2Config.h>
#include <codec2/aidl/Configurable.h>
#include <codec2/aidl/ParamTypes.h>

#include "component.h"
#include "decoder_interface.h"
#include "host_link.h"
#include "log.h"

namespace dsa::codec2 {

using c2aidl::IComponentStore;
using c2aidl::utils::ConfigurableC2Intf;
using c2aidl::utils::ToAidl;

namespace {

// How long the first list of components waits for the host to say what it decodes. The
// framework builds its list of codecs once a boot, so a store that answers before the host
// has said would leave the host's decoders out until the next boot.
constexpr auto kFirstAnswerWait = std::chrono::seconds(20);
// How long a component waits, when the host has not said yet.
constexpr auto kComponentWait = std::chrono::seconds(2);
// How often the store asks a host that has not answered.
constexpr auto kAskEvery = std::chrono::seconds(2);
// The rank of the host's decoders: ahead of the guest's software decoders (0x200), so the
// framework picks them first and keeps the software ones to fall back on.
constexpr int32_t kRank = 0x80;

ndk::ScopedAStatus status(c2_status_t error) { return ndk::ScopedAStatus::fromServiceSpecificError(error); }

// The store's own parameters: how the framework allocates the linear memory of codec buffers.
//
// The framework makes this store, the device's default one, the preferred store of every
// process that uses a codec, and asks it how to allocate linear memory (the input of every
// decoder, software ones included, and the output of audio decoders): which DMA-BUF heap, or
// which ion heaps, for a usage. Once the store says it knows the fields, each allocation asks
// it with config, and a refused config fails the allocation. A store that answered the
// question and refused the config failed every MediaCodec.start() with NO_MEMORY, in every
// app. The answers are the platform's own store's: the system heap, no flags, no alignment.
class StoreParams : public C2InterfaceHelper {
   public:
    explicit StoreParams(const std::shared_ptr<C2ReflectorHelper>& reflector) : C2InterfaceHelper(reflector) {
        setDerivedInstance(this);

        addParameter(DefineParam(mIonUsage, "ion-usage")
                         .withDefault(new C2StoreIonUsageInfo())
                         .withFields({
                             C2F(mIonUsage, usage).flags({C2MemoryUsage::CPU_READ | C2MemoryUsage::CPU_WRITE}),
                             C2F(mIonUsage, capacity).inRange(0, UINT32_MAX, 1024),
                             C2F(mIonUsage, heapMask).any(),
                             C2F(mIonUsage, allocFlags).flags({}),
                             C2F(mIonUsage, minAlignment).equalTo(0),
                         })
                         .withSetter(SetIonUsage)
                         .build());

        addParameter(DefineParam(mDmaBufUsage, "dmabuf-usage")
                         .withDefault(C2StoreDmaBufUsageInfo::AllocShared(0))
                         .withFields({
                             C2F(mDmaBufUsage, m.usage).flags({C2MemoryUsage::CPU_READ | C2MemoryUsage::CPU_WRITE}),
                             C2F(mDmaBufUsage, m.capacity).inRange(0, UINT32_MAX, 1024),
                             C2F(mDmaBufUsage, m.allocFlags).flags({}),
                             C2F(mDmaBufUsage, m.heapName).any(),
                         })
                         .withSetter(SetDmaBufUsage)
                         .build());
    }

    // Any ion heap, with no flags. The guest's kernel has no ion, so the framework takes the
    // DMA-BUF heaps; the answer is here for a framework that asks anyway.
    static C2R SetIonUsage(bool, C2P<C2StoreIonUsageInfo>& me) {
        me.set().heapMask = ~0u;
        me.set().allocFlags = 0;
        me.set().minAlignment = 0;
        return C2R::Ok();
    }

    // The system heap (/dev/dma_heap/system), the only DMA-BUF heap the guest's kernel has.
    static C2R SetDmaBufUsage(bool, C2P<C2StoreDmaBufUsageInfo>& me) {
        strncpy(me.set().m.heapName, kSystemHeap, me.v.flexCount());
        me.set().m.allocFlags = 0;
        return C2R::Ok();
    }

   private:
    static constexpr const char* kSystemHeap = "system";

    std::shared_ptr<C2StoreIonUsageInfo> mIonUsage;
    std::shared_ptr<C2StoreDmaBufUsageInfo> mDmaBufUsage;
};

// The store's parameters as the HAL's configurable serves them.
struct StoreIntf : public ConfigurableC2Intf {
    explicit StoreIntf(const std::shared_ptr<C2ReflectorHelper>& reflector)
        : ConfigurableC2Intf("dsa.codec2.store", 0), mParams(std::make_shared<StoreParams>(reflector)) {}
    c2_status_t query(const std::vector<C2Param::Index>& indices, c2_blocking_t mayBlock,
                      std::vector<std::unique_ptr<C2Param>>* const params) const override {
        return mParams->query({}, indices, mayBlock, params);
    }
    c2_status_t config(const std::vector<C2Param*>& params, c2_blocking_t mayBlock,
                       std::vector<std::unique_ptr<C2SettingResult>>* const failures) override {
        return mParams->config(params, mayBlock, failures);
    }
    c2_status_t querySupportedParams(std::vector<std::shared_ptr<C2ParamDescriptor>>* const params) const override {
        return mParams->querySupportedParams(params);
    }
    c2_status_t querySupportedValues(std::vector<C2FieldSupportedValuesQuery>& fields,
                                     c2_blocking_t mayBlock) const override {
        return mParams->querySupportedValues(fields, mayBlock);
    }

    std::shared_ptr<StoreParams> mParams;
};

struct NoCache : public c2aidl::utils::ParameterCache {
    c2_status_t validate(const std::vector<std::shared_ptr<C2ParamDescriptor>>&) override { return C2_OK; }
};

}  // namespace

Store::Store() : mReflector(std::make_shared<C2ReflectorHelper>()) {
    auto configurable =
        ndk::SharedRefBase::make<c2aidl::utils::CachedConfigurable>(std::make_unique<StoreIntf>(mReflector));
    configurable->init(std::make_shared<NoCache>());
    mConfigurable = configurable;
}

Store::~Store() {
    {
        std::lock_guard<std::mutex> lock(mLock);
        mStopping = true;
    }
    mAnswered.notify_all();
    if (mAsker.joinable()) {
        mAsker.join();
    }
}

void Store::start() { mAsker = std::thread([this] { ask(); }); }

void Store::ask() {
    bool warned = false;
    while (true) {
        auto caps = queryCapabilities();
        std::unique_lock<std::mutex> lock(mLock);
        if (caps) {
            mCaps = std::move(caps);
            for (const CodecSupport& support : *mCaps) {
                CODEC_LOGI("the host decodes format %d up to %ux%u on its GPU", static_cast<int>(support.codec),
                           support.maxWidth, support.maxHeight);
            }
            if (mCaps->empty()) {
                CODEC_LOGI("the host's GPU decodes none of the formats; the guest's own decoders are used");
            }
            mAnswered.notify_all();
            return;
        }
        if (!warned) {
            CODEC_LOGI("waiting for the host's video port");
            warned = true;
        }
        if (mAnswered.wait_for(lock, kAskEvery, [this] { return mStopping; })) {
            return;
        }
    }
}

std::optional<std::vector<CodecSupport>> Store::capabilities(std::chrono::milliseconds wait) {
    std::unique_lock<std::mutex> lock(mLock);
    mAnswered.wait_for(lock, wait, [this] { return mCaps.has_value() || mStopping; });
    return mCaps;
}

std::optional<CodecSupport> Store::supportFor(const std::string& name) {
    const DecoderInfo* info = findDecoder(name);
    if (info == nullptr) {
        return std::nullopt;
    }
    const auto caps = capabilities(kComponentWait);
    if (!caps) {
        return std::nullopt;
    }
    for (const CodecSupport& support : *caps) {
        if (support.codec == info->codec) {
            return support;
        }
    }
    return std::nullopt;
}

ndk::ScopedAStatus Store::listComponents(std::vector<IComponentStore::ComponentTraits>* traits) {
    const auto caps = capabilities(kFirstAnswerWait);
    traits->clear();
    if (!caps) {
        CODEC_LOGW("the host has not said what it decodes; no decoder is offered");
        return ndk::ScopedAStatus::ok();
    }
    size_t count = 0;
    const DecoderInfo* infos = decoderInfos(&count);
    for (size_t i = 0; i < count; ++i) {
        for (const CodecSupport& support : *caps) {
            if (support.codec != infos[i].codec) {
                continue;
            }
            IComponentStore::ComponentTraits trait;
            trait.name = infos[i].name;
            trait.domain = IComponentStore::ComponentTraits::Domain::VIDEO;
            trait.kind = IComponentStore::ComponentTraits::Kind::DECODER;
            trait.rank = kRank;
            trait.mediaType = infos[i].mediaType;
            traits->push_back(std::move(trait));
        }
    }
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Store::createComponent(
    const std::string& name, const std::shared_ptr<c2aidl::IComponentListener>& listener,
    const std::shared_ptr<::aidl::android::hardware::media::bufferpool2::IClientManager>&,
    std::shared_ptr<c2aidl::IComponent>* component) {
    const auto support = supportFor(name);
    if (!support) {
        return status(C2_NOT_FOUND);
    }
    std::shared_ptr<DecoderIntf> params;
    auto intf = makeDecoderInterface(mReflector, *findDecoder(name), *support, &params);
    *component = ndk::SharedRefBase::make<Component>(*findDecoder(name), intf, params, listener);
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Store::createInterface(const std::string& name,
                                          std::shared_ptr<c2aidl::IComponentInterface>* intf) {
    const auto support = supportFor(name);
    if (!support) {
        return status(C2_NOT_FOUND);
    }
    std::shared_ptr<DecoderIntf> params;
    *intf = ndk::SharedRefBase::make<ComponentInterface>(
        makeDecoderInterface(mReflector, *findDecoder(name), *support, &params));
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Store::getConfigurable(std::shared_ptr<c2aidl::IConfigurable>* configurable) {
    *configurable = mConfigurable;
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Store::getPoolClientManager(
    std::shared_ptr<::aidl::android::hardware::media::bufferpool2::IClientManager>* manager) {
    // The guest image keeps the framework's codec buffers out of buffer pools, so none is
    // needed.
    *manager = nullptr;
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Store::getStructDescriptors(const std::vector<int32_t>& indices,
                                               std::vector<c2aidl::StructDescriptor>* descriptors) {
    descriptors->clear();
    for (int32_t index : indices) {
        const C2Param::CoreIndex coreIndex = C2Param::CoreIndex(static_cast<uint32_t>(index)).coreIndex();
        std::unique_ptr<C2StructDescriptor> described = mReflector->describe(coreIndex);
        if (!described) {
            return status(C2_NOT_FOUND);
        }
        c2aidl::StructDescriptor descriptor;
        if (!ToAidl(&descriptor, *described)) {
            return status(C2_CORRUPTED);
        }
        descriptors->push_back(std::move(descriptor));
    }
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Store::copyBuffer(const c2aidl::Buffer&, const c2aidl::Buffer&) {
    return ndk::ScopedAStatus::fromExceptionCode(EX_UNSUPPORTED_OPERATION);
}

ndk::ScopedAStatus Store::createInputSurface(std::shared_ptr<c2aidl::IInputSurface>* surface) {
    *surface = nullptr;
    return ndk::ScopedAStatus::ok();
}

}  // namespace dsa::codec2
