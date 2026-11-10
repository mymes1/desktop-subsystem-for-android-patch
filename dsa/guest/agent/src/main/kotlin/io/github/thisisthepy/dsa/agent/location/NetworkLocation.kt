package io.github.thisisthepy.dsa.agent.location

import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.LocationAccess
import io.github.thisisthepy.dsa.control.v1.LocationFix
import io.github.thisisthepy.dsa.control.v1.LocationRequest

/** One position from the host, as the guest's network location provider reports it. */
data class HostFix(
    val latitudeDegrees: Double,
    val longitudeDegrees: Double,
    /** The radius of 68% confidence, in meters; never 0. */
    val accuracyMeters: Float,
    val altitudeMeters: Double?,
    val verticalAccuracyMeters: Float?,
    val speedMetersPerSecond: Float?,
    val bearingDegrees: Float?,
    /** When the host took it, in milliseconds since the Unix epoch. */
    val unixTimeMs: Long,
)

/** The provider as Android sees it: the platform's network location provider. */
interface NetworkLocationSink {
    /** A position for Android's `network` provider. */
    fun report(fix: HostFix)

    /** Whether the host lets Android have its location now. */
    fun setAllowed(allowed: Boolean)

    companion object {
        val NONE = object : NetworkLocationSink {
            override fun report(fix: HostFix) = Unit
            override fun setAllowed(allowed: Boolean) = Unit
        }
    }
}

/**
 * The guest's network location, from the host's location.
 *
 * Android's location manager binds the agent's network location provider, and asks it for
 * locations ([request]) while an app with a location permission wants network locations and
 * the user has location on in Android. The agent asks the host for its location with
 * `LocationRequest`, as the GNSS HAL does; the control daemon tells the host what both want
 * together. Each `LocationFix` the host sends then reaches the provider ([NetworkLocationSink])
 * as a network location, and each `LocationStatus` says whether the host allows it, so a
 * location the user turned off in the host's privacy settings reads as a disabled provider in
 * Android. Fixes that come while the provider wants none (for the GNSS HAL) are dropped here.
 *
 * Everything runs on the agent's thread.
 */
class NetworkLocation(
    private val send: (ByteArray) -> Boolean,
    private val log: Log,
    private val isConnected: () -> Boolean,
) {
    /** The provider, while Android has it bound. */
    var sink: NetworkLocationSink = NetworkLocationSink.NONE

    /** The interval Android wants locations at, in milliseconds, while it wants any. */
    private var interval: Long? = null

    /** Whether locations are wanted now. */
    val active: Boolean get() = interval != null

    /**
     * Android wants locations every [intervalMillis] ([active]), or none. Sends the host the
     * change; a request the host already has is not sent again.
     */
    fun request(active: Boolean, intervalMillis: Long) {
        val wanted = if (active) intervalMillis.coerceIn(MIN_INTERVAL_MS, MAX_INTERVAL_MS) else null
        if (wanted == interval) return
        interval = wanted
        if (wanted != null) {
            log.info("network location: Android wants the host's location every $wanted ms")
        } else {
            log.info("network location: Android wants no location")
        }
        if (isConnected()) tell()
    }

    /** The channel is up again: the host is told what Android wants, if it wants any. */
    fun connected() {
        if (interval != null) tell()
    }

    /** Takes the host's location frames. They go to the GNSS HAL too, so none is consumed. */
    fun handle(envelope: Envelope): Boolean {
        when (envelope.bodyCase) {
            Envelope.BodyCase.LOCATION_FIX -> if (interval != null) {
                val fix = toFix(envelope.locationFix)
                if (fix != null) sink.report(fix)
            }
            Envelope.BodyCase.LOCATION_STATUS -> if (interval != null) {
                val allowed = envelope.locationStatus.access == LocationAccess.LOCATION_ACCESS_ALLOWED
                if (!allowed) {
                    log.info("network location: the host gives no location (${envelope.locationStatus.detail})")
                }
                sink.setAllowed(allowed)
            }
            else -> Unit
        }
        return false
    }

    private fun tell() {
        val wanted = interval
        val request = LocationRequest.newBuilder()
            .setActive(wanted != null)
            .setIntervalMs((wanted ?: 0L).toInt())
            .build()
        send(Envelope.newBuilder().setLocationRequest(request).build().toByteArray())
    }

    companion object {
        /** No network location comes faster than once a second. */
        const val MIN_INTERVAL_MS = 1_000L

        /** The longest interval passed to the host, an hour; Android's "passive" is none. */
        const val MAX_INTERVAL_MS = 3_600_000L

        /** The accuracy a position the host gave none for is reported with, in meters. */
        const val UNKNOWN_ACCURACY_METERS = 500f

        /** The host's fix as a network location, or null when it is not a position on Earth. */
        fun toFix(fix: LocationFix): HostFix? {
            val lat = fix.latitudeDegrees
            val lon = fix.longitudeDegrees
            if (!lat.isFinite() || !lon.isFinite() || lat !in -90.0..90.0 || lon !in -180.0..180.0) return null
            val accuracy = fix.horizontalAccuracyMeters
            return HostFix(
                latitudeDegrees = lat,
                longitudeDegrees = lon,
                accuracyMeters = if (accuracy.isFinite() && accuracy > 0f) accuracy else UNKNOWN_ACCURACY_METERS,
                altitudeMeters = if (fix.hasAltitude) fix.altitudeMeters else null,
                verticalAccuracyMeters = fix.verticalAccuracyMeters.takeIf { fix.hasAltitude && it > 0f },
                speedMetersPerSecond = if (fix.hasSpeed) fix.speedMetersPerSecond else null,
                bearingDegrees = if (fix.hasBearing) fix.bearingDegrees else null,
                unixTimeMs = fix.unixTimeMs,
            )
        }
    }
}
