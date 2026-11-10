package io.github.thisisthepy.dsa.agent.platform

import android.app.Service
import android.content.Intent
import android.location.Location
import android.location.LocationManager
import android.location.provider.LocationProviderBase
import android.location.provider.ProviderProperties
import android.location.provider.ProviderRequest
import android.os.Bundle
import android.os.IBinder
import android.os.SystemClock
import io.github.thisisthepy.dsa.agent.location.HostFix
import io.github.thisisthepy.dsa.agent.location.NetworkLocationSink

/**
 * The guest's network location provider. The image names the agent as the provider
 * (`config_networkLocationProviderPackageName`), so Android's location manager binds this
 * service for the `network` provider, whether or not Google Play is installed. It asks the
 * host for its location while Android wants network locations, which Android does only for
 * apps that hold a location permission and while the user has location on, and reports each
 * position the host gives (the agent's
 * [io.github.thisisthepy.dsa.agent.location.NetworkLocation]). While the host's privacy
 * settings keep the location from DSA, the provider is not allowed, which apps see as a
 * disabled provider.
 */
class HostNetworkLocationService : Service() {
    private var provider: Provider? = null

    override fun onBind(intent: Intent?): IBinder? {
        val bound = provider ?: Provider().also { provider = it }
        AgentApplication.agent?.networkLocationProvider(bound)
        return bound.getBinder()
    }

    override fun onUnbind(intent: Intent?): Boolean {
        AgentApplication.agent?.networkLocationProvider(null)
        return false
    }

    override fun onDestroy() {
        AgentApplication.agent?.networkLocationProvider(null)
        provider = null
        super.onDestroy()
    }

    private inner class Provider :
        LocationProviderBase(this@HostNetworkLocationService, TAG, properties()),
        NetworkLocationSink {
        init {
            // Not allowed until the host says it gives its location.
            setAllowed(false)
        }

        override fun onSetRequest(request: ProviderRequest) {
            val agent = AgentApplication.agent ?: return
            val active = request.isActive && request.intervalMillis != ProviderRequest.INTERVAL_DISABLED
            agent.wantNetworkLocation(active, request.intervalMillis)
        }

        override fun onFlush(callback: OnFlushCompleteCallback) {
            // Nothing is batched: each position is reported as it comes.
            callback.onFlushComplete()
        }

        override fun onSendExtraCommand(command: String, extras: Bundle?) = Unit

        override fun report(fix: HostFix) {
            val location = Location(LocationManager.NETWORK_PROVIDER).apply {
                latitude = fix.latitudeDegrees
                longitude = fix.longitudeDegrees
                accuracy = fix.accuracyMeters
                fix.altitudeMeters?.let { altitude = it }
                fix.verticalAccuracyMeters?.let { verticalAccuracyMeters = it }
                fix.speedMetersPerSecond?.let { speed = it }
                fix.bearingDegrees?.let { bearing = it }
                // The host's clock and the guest's agree; a position from the host's past keeps
                // its age on the guest's elapsed-time clock.
                val now = System.currentTimeMillis()
                val age = (now - fix.unixTimeMs).coerceIn(0L, MAX_AGE_MS)
                time = if (fix.unixTimeMs > 0) fix.unixTimeMs else now
                elapsedRealtimeNanos = SystemClock.elapsedRealtimeNanos() - age * 1_000_000L
            }
            if (!isAllowed) setAllowed(true)
            reportLocation(location)
        }

        override fun setAllowed(allowed: Boolean) {
            super.setAllowed(allowed)
        }
    }

    private companion object {
        const val TAG = "DsaNetworkLocation"

        /** The oldest a position's age is taken to be, so a wrong clock cannot make it ancient. */
        const val MAX_AGE_MS = 60_000L

        fun properties(): ProviderProperties = ProviderProperties.Builder()
            .setHasNetworkRequirement(true)
            .setAccuracy(ProviderProperties.ACCURACY_COARSE)
            .setPowerUsage(ProviderProperties.POWER_USAGE_LOW)
            .build()
    }
}
