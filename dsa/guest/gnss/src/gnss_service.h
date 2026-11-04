// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
//
// dsa-gnss's IGnss (android.hardware.gnss, version 4). The framework's GNSS location provider
// starts and stops the engine here; between the two, the host's location service runs and
// each position it finds reaches the framework through gnssLocationCb. The host's position
// has no satellites, measurements or navigation messages, so those extensions are not
// offered, and neither is the rest of what a phone's GNSS chip does (assistance data,
// geofencing, batching): the framework does without them.

#pragma once

#include <functional>
#include <memory>
#include <mutex>
#include <vector>

#include <aidl/android/hardware/gnss/BnGnss.h>
#include <aidl/android/hardware/gnss/IGnssCallback.h>

#include "location_session.h"

namespace dsa::gnss {

class Gnss : public aidl::android::hardware::gnss::BnGnss {
   public:
    using Sender = std::function<bool(const std::vector<uint8_t>&)>;

    explicit Gnss(Sender send) : mSend(std::move(send)) {}

    // A frame from the host, which may carry a position or the host's location status.
    void hostFrame(const std::vector<uint8_t>& frame);
    // The channel subscribed again: the host is told again that a location is wanted.
    void channelConnected();

    ndk::ScopedAStatus setCallback(
        const std::shared_ptr<aidl::android::hardware::gnss::IGnssCallback>& callback) override;
    ndk::ScopedAStatus close() override;
    ndk::ScopedAStatus getExtensionPsds(
        std::shared_ptr<aidl::android::hardware::gnss::IGnssPsds>* psds) override;
    ndk::ScopedAStatus getExtensionGnssConfiguration(
        std::shared_ptr<aidl::android::hardware::gnss::IGnssConfiguration>* configuration) override;
    ndk::ScopedAStatus getExtensionGnssMeasurement(
        std::shared_ptr<aidl::android::hardware::gnss::IGnssMeasurementInterface>* measurement) override;
    ndk::ScopedAStatus getExtensionGnssPowerIndication(
        std::shared_ptr<aidl::android::hardware::gnss::IGnssPowerIndication>* power) override;
    ndk::ScopedAStatus getExtensionGnssBatching(
        std::shared_ptr<aidl::android::hardware::gnss::IGnssBatching>* batching) override;
    ndk::ScopedAStatus getExtensionGnssGeofence(
        std::shared_ptr<aidl::android::hardware::gnss::IGnssGeofence>* geofence) override;
    ndk::ScopedAStatus getExtensionGnssNavigationMessage(
        std::shared_ptr<aidl::android::hardware::gnss::IGnssNavigationMessageInterface>* navigation) override;
    ndk::ScopedAStatus getExtensionAGnss(std::shared_ptr<aidl::android::hardware::gnss::IAGnss>* agnss) override;
    ndk::ScopedAStatus getExtensionAGnssRil(
        std::shared_ptr<aidl::android::hardware::gnss::IAGnssRil>* ril) override;
    ndk::ScopedAStatus getExtensionGnssDebug(
        std::shared_ptr<aidl::android::hardware::gnss::IGnssDebug>* debug) override;
    ndk::ScopedAStatus getExtensionGnssVisibilityControl(
        std::shared_ptr<aidl::android::hardware::gnss::visibility_control::IGnssVisibilityControl>* control) override;
    ndk::ScopedAStatus start() override;
    ndk::ScopedAStatus stop() override;
    ndk::ScopedAStatus injectTime(int64_t timeMs, int64_t timeReferenceMs, int32_t uncertaintyMs) override;
    ndk::ScopedAStatus injectLocation(const aidl::android::hardware::gnss::GnssLocation& location) override;
    ndk::ScopedAStatus injectBestLocation(const aidl::android::hardware::gnss::GnssLocation& location) override;
    ndk::ScopedAStatus deleteAidingData(aidl::android::hardware::gnss::IGnss::GnssAidingData flags) override;
    ndk::ScopedAStatus setPositionMode(
        const aidl::android::hardware::gnss::IGnss::PositionModeOptions& options) override;
    ndk::ScopedAStatus getExtensionGnssAntennaInfo(
        std::shared_ptr<aidl::android::hardware::gnss::IGnssAntennaInfo>* antenna) override;
    ndk::ScopedAStatus getExtensionMeasurementCorrections(
        std::shared_ptr<aidl::android::hardware::gnss::measurement_corrections::IMeasurementCorrectionsInterface>*
            corrections) override;
    ndk::ScopedAStatus startSvStatus() override;
    ndk::ScopedAStatus stopSvStatus() override;
    ndk::ScopedAStatus startNmea() override;
    ndk::ScopedAStatus stopNmea() override;

   private:
    void reportStatus(aidl::android::hardware::gnss::IGnssCallback::GnssStatusValue status);

    Sender mSend;
    // Held around every change to the session and the frame it sends, so the host hears the
    // framework's starts and stops in order.
    std::mutex mLock;
    LocationSession mSession;
    std::shared_ptr<aidl::android::hardware::gnss::IGnssCallback> mCallback;
};

}  // namespace dsa::gnss
