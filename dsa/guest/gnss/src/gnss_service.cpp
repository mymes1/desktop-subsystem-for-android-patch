// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

#include "gnss_service.h"

#include <time.h>

#include "log.h"

namespace dsa::gnss {

using aidl::android::hardware::gnss::ElapsedRealtime;
using aidl::android::hardware::gnss::GnssLocation;
using aidl::android::hardware::gnss::IAGnss;
using aidl::android::hardware::gnss::IAGnssRil;
using aidl::android::hardware::gnss::IGnss;
using aidl::android::hardware::gnss::IGnssAntennaInfo;
using aidl::android::hardware::gnss::IGnssBatching;
using aidl::android::hardware::gnss::IGnssCallback;
using aidl::android::hardware::gnss::IGnssConfiguration;
using aidl::android::hardware::gnss::IGnssDebug;
using aidl::android::hardware::gnss::IGnssGeofence;
using aidl::android::hardware::gnss::IGnssMeasurementInterface;
using aidl::android::hardware::gnss::IGnssNavigationMessageInterface;
using aidl::android::hardware::gnss::IGnssPowerIndication;
using aidl::android::hardware::gnss::IGnssPsds;
using aidl::android::hardware::gnss::measurement_corrections::IMeasurementCorrectionsInterface;
using aidl::android::hardware::gnss::visibility_control::IGnssVisibilityControl;

namespace {

// An extension the framework may do without: it logs that it is missing and goes on.
ndk::ScopedAStatus notOffered() { return ndk::ScopedAStatus::fromExceptionCode(EX_UNSUPPORTED_OPERATION); }

// The time since boot, as the framework's elapsedRealtimeNanos counts it.
int64_t elapsedRealtimeNanos() {
    struct timespec now {};
    clock_gettime(CLOCK_BOOTTIME, &now);
    return static_cast<int64_t>(now.tv_sec) * 1000000000LL + now.tv_nsec;
}

GnssLocation toGnssLocation(const Fix& fix) {
    GnssLocation location;
    location.gnssLocationFlags = fix.flags;
    location.latitudeDegrees = fix.latitudeDegrees;
    location.longitudeDegrees = fix.longitudeDegrees;
    location.altitudeMeters = fix.altitudeMeters;
    location.speedMetersPerSec = fix.speedMetersPerSec;
    location.bearingDegrees = fix.bearingDegrees;
    location.horizontalAccuracyMeters = fix.horizontalAccuracyMeters;
    location.verticalAccuracyMeters = fix.verticalAccuracyMeters;
    location.timestampMillis = fix.timestampMillis;
    // The position reached the guest just now; the framework ages it from here.
    location.elapsedRealtime.flags =
        ElapsedRealtime::HAS_TIMESTAMP_NS | ElapsedRealtime::HAS_TIME_UNCERTAINTY_NS;
    location.elapsedRealtime.timestampNs = elapsedRealtimeNanos();
    // The control channel's delay, which is well under this.
    location.elapsedRealtime.timeUncertaintyNs = 50'000'000.0;
    return location;
}

}  // namespace

void Gnss::hostFrame(const std::vector<uint8_t>& frame) {
    const std::optional<HostLocation> location = decodeHostLocation(frame.data(), frame.size());
    if (!location) {
        return;
    }
    std::shared_ptr<IGnssCallback> callback;
    std::optional<Fix> fix;
    {
        std::lock_guard<std::mutex> lock(mLock);
        const Access before = mSession.access();
        fix = mSession.fromHost(*location);
        if (location->access && *location->access != before) {
            if (location->detail.empty()) {
                GNSS_LOGI("the host's location is %s", describe(*location->access));
            } else {
                GNSS_LOGW("the host's location is %s: %s", describe(*location->access),
                          location->detail.c_str());
            }
        }
        callback = mCallback;
    }
    if (fix && callback) {
        const ndk::ScopedAStatus status = callback->gnssLocationCb(toGnssLocation(*fix));
        if (!status.isOk()) {
            GNSS_LOGE("the framework refused a location: %s", status.getDescription().c_str());
        }
    }
}

void Gnss::channelConnected() {
    std::lock_guard<std::mutex> lock(mLock);
    if (const auto request = mSession.reconnected()) {
        mSend(*request);
        GNSS_LOGI("told the host again that a location is wanted");
    }
}

void Gnss::reportStatus(IGnssCallback::GnssStatusValue status) {
    std::shared_ptr<IGnssCallback> callback;
    {
        std::lock_guard<std::mutex> lock(mLock);
        callback = mCallback;
    }
    if (callback) {
        callback->gnssStatusCb(status);
    }
}

ndk::ScopedAStatus Gnss::setCallback(const std::shared_ptr<IGnssCallback>& callback) {
    if (!callback) {
        return ndk::ScopedAStatus::fromExceptionCode(EX_NULL_POINTER);
    }
    {
        std::lock_guard<std::mutex> lock(mLock);
        mCallback = callback;
    }
    // The engine honours the interval the framework sets; nothing else a GNSS chip offers.
    callback->gnssSetCapabilitiesCb(IGnssCallback::CAPABILITY_SCHEDULING);
    IGnssCallback::GnssSystemInfo info;
    info.yearOfHw = 2026;
    info.name = "DSA host location";
    callback->gnssSetSystemInfoCb(info);
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Gnss::close() {
    {
        std::lock_guard<std::mutex> lock(mLock);
        if (const auto request = mSession.stop()) {
            mSend(*request);
        }
        mCallback.reset();
    }
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Gnss::start() {
    {
        std::lock_guard<std::mutex> lock(mLock);
        mSend(mSession.start());
        GNSS_LOGI("the framework wants the location every %u ms; asked the host", mSession.intervalMs());
    }
    reportStatus(IGnssCallback::GnssStatusValue::SESSION_BEGIN);
    reportStatus(IGnssCallback::GnssStatusValue::ENGINE_ON);
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Gnss::stop() {
    bool stopped = false;
    {
        std::lock_guard<std::mutex> lock(mLock);
        if (const auto request = mSession.stop()) {
            mSend(*request);
            stopped = true;
            GNSS_LOGI("the framework wants no location; told the host");
        }
    }
    if (stopped) {
        reportStatus(IGnssCallback::GnssStatusValue::SESSION_END);
        reportStatus(IGnssCallback::GnssStatusValue::ENGINE_OFF);
    }
    return ndk::ScopedAStatus::ok();
}

ndk::ScopedAStatus Gnss::setPositionMode(const IGnss::PositionModeOptions& options) {
    std::lock_guard<std::mutex> lock(mLock);
    const uint32_t interval = options.minIntervalMs > 0 ? static_cast<uint32_t>(options.minIntervalMs) : 0;
    if (const auto request = mSession.setInterval(interval)) {
        mSend(*request);
    }
    return ndk::ScopedAStatus::ok();
}

// The host's position needs no help: time and positions the framework injects, and its
// requests to forget assistance data, change nothing.
ndk::ScopedAStatus Gnss::injectTime(int64_t, int64_t, int32_t) { return ndk::ScopedAStatus::ok(); }
ndk::ScopedAStatus Gnss::injectLocation(const GnssLocation&) { return ndk::ScopedAStatus::ok(); }
ndk::ScopedAStatus Gnss::injectBestLocation(const GnssLocation&) { return ndk::ScopedAStatus::ok(); }
ndk::ScopedAStatus Gnss::deleteAidingData(IGnss::GnssAidingData) { return ndk::ScopedAStatus::ok(); }

// There are no satellites to report, and no NMEA sentences.
ndk::ScopedAStatus Gnss::startSvStatus() { return ndk::ScopedAStatus::ok(); }
ndk::ScopedAStatus Gnss::stopSvStatus() { return ndk::ScopedAStatus::ok(); }
ndk::ScopedAStatus Gnss::startNmea() { return ndk::ScopedAStatus::ok(); }
ndk::ScopedAStatus Gnss::stopNmea() { return ndk::ScopedAStatus::ok(); }

// The extensions the interface marks optional: none, and the call succeeds.
ndk::ScopedAStatus Gnss::getExtensionPsds(std::shared_ptr<IGnssPsds>* psds) {
    *psds = nullptr;
    return ndk::ScopedAStatus::ok();
}
ndk::ScopedAStatus Gnss::getExtensionGnssBatching(std::shared_ptr<IGnssBatching>* batching) {
    *batching = nullptr;
    return ndk::ScopedAStatus::ok();
}
ndk::ScopedAStatus Gnss::getExtensionGnssGeofence(std::shared_ptr<IGnssGeofence>* geofence) {
    *geofence = nullptr;
    return ndk::ScopedAStatus::ok();
}
ndk::ScopedAStatus Gnss::getExtensionGnssNavigationMessage(
    std::shared_ptr<IGnssNavigationMessageInterface>* navigation) {
    *navigation = nullptr;
    return ndk::ScopedAStatus::ok();
}
ndk::ScopedAStatus Gnss::getExtensionMeasurementCorrections(
    std::shared_ptr<IMeasurementCorrectionsInterface>* corrections) {
    *corrections = nullptr;
    return ndk::ScopedAStatus::ok();
}

// The extensions the interface returns as present: not offered, which the framework treats
// as a GNSS chip without that feature.
ndk::ScopedAStatus Gnss::getExtensionGnssConfiguration(std::shared_ptr<IGnssConfiguration>*) { return notOffered(); }
ndk::ScopedAStatus Gnss::getExtensionGnssMeasurement(std::shared_ptr<IGnssMeasurementInterface>*) {
    return notOffered();
}
ndk::ScopedAStatus Gnss::getExtensionGnssPowerIndication(std::shared_ptr<IGnssPowerIndication>*) {
    return notOffered();
}
ndk::ScopedAStatus Gnss::getExtensionAGnss(std::shared_ptr<IAGnss>*) { return notOffered(); }
ndk::ScopedAStatus Gnss::getExtensionAGnssRil(std::shared_ptr<IAGnssRil>*) { return notOffered(); }
ndk::ScopedAStatus Gnss::getExtensionGnssDebug(std::shared_ptr<IGnssDebug>*) { return notOffered(); }
ndk::ScopedAStatus Gnss::getExtensionGnssVisibilityControl(std::shared_ptr<IGnssVisibilityControl>*) {
    return notOffered();
}
ndk::ScopedAStatus Gnss::getExtensionGnssAntennaInfo(std::shared_ptr<IGnssAntennaInfo>*) { return notOffered(); }

}  // namespace dsa::gnss
