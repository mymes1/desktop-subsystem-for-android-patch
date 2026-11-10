package io.github.thisisthepy.dsa.agent.platform

import android.app.Service
import android.content.Intent
import android.location.Address
import android.location.provider.ForwardGeocodeRequest
import android.location.provider.GeocodeProviderBase
import android.location.provider.ReverseGeocodeRequest
import android.os.IBinder
import android.os.OutcomeReceiver
import io.github.thisisthepy.dsa.agent.geocode.GeoBounds
import io.github.thisisthepy.dsa.agent.geocode.GeocodeAnswer
import io.github.thisisthepy.dsa.agent.geocode.GeocodeFailure
import io.github.thisisthepy.dsa.agent.geocode.GeocodeLookup
import io.github.thisisthepy.dsa.agent.geocode.HostAddress
import java.io.IOException
import java.util.Locale

/**
 * The guest's geocode provider while Google Play is not installed. The image names the agent as
 * the provider (`config_geocoderProviderPackageName`), so Android's `Geocoder` is present and
 * every lookup an app makes comes here; on a boot with the Google apps image an overlay names
 * Google's geocoder instead, and this service is not bound. Each lookup goes to the host
 * ([io.github.thisisthepy.dsa.agent.geocode.HostGeocoder]), which answers a position with its
 * country, province, city, district and neighbourhood from boundary data it keeps, and an
 * address with no positions. What the host found comes back as Android `Address`es. A failure
 * reaches the app as Android's geocoder reports one: an
 * `IOException` (the service is not available, or too busy), or an
 * `IllegalArgumentException` for a lookup that cannot be answered.
 */
class HostGeocodeService : Service() {
    private val provider by lazy { Provider() }

    override fun onBind(intent: Intent?): IBinder = provider.getBinder()

    private inner class Provider : GeocodeProviderBase(this@HostGeocodeService, TAG) {
        override fun onForwardGeocode(
            request: ForwardGeocodeRequest,
            callback: OutcomeReceiver<List<Address>, Throwable>,
        ) {
            val bounds = GeoBounds(
                request.lowerLeftLatitude,
                request.lowerLeftLongitude,
                request.upperRightLatitude,
                request.upperRightLongitude,
            ).takeIf { it.lowerLeftLatitude != it.upperRightLatitude || it.lowerLeftLongitude != it.upperRightLongitude }
            val locale = request.locale ?: Locale.getDefault()
            lookup(
                GeocodeLookup.Forward(request.locationName ?: "", bounds, request.maxResults, locale.toLanguageTag()),
                locale,
                callback,
            )
        }

        override fun onReverseGeocode(
            request: ReverseGeocodeRequest,
            callback: OutcomeReceiver<List<Address>, Throwable>,
        ) {
            val locale = request.locale ?: Locale.getDefault()
            lookup(
                GeocodeLookup.Reverse(request.latitude, request.longitude, request.maxResults, locale.toLanguageTag()),
                locale,
                callback,
            )
        }

        private fun lookup(
            lookup: GeocodeLookup,
            locale: Locale,
            callback: OutcomeReceiver<List<Address>, Throwable>,
        ) {
            val agent = AgentApplication.agent
            if (agent == null) {
                callback.onError(IOException("Service not Available: DSA's agent is not running"))
                return
            }
            agent.geocode(lookup, object : GeocodeAnswer {
                override fun found(addresses: List<HostAddress>) {
                    callback.onResult(addresses.map { toAndroid(it, locale) })
                }

                override fun failed(failure: GeocodeFailure, message: String) {
                    callback.onError(toError(failure, message))
                }
            })
        }
    }

    private companion object {
        const val TAG = "DsaGeocoder"

        fun toAndroid(address: HostAddress, locale: Locale): Address = Address(locale).apply {
            address.addressLines.forEachIndexed { index, line -> setAddressLine(index, line) }
            featureName = address.featureName.ifEmpty { null }
            premises = address.premises.ifEmpty { null }
            subThoroughfare = address.subThoroughfare.ifEmpty { null }
            thoroughfare = address.thoroughfare.ifEmpty { null }
            subLocality = address.subLocality.ifEmpty { null }
            locality = address.locality.ifEmpty { null }
            subAdminArea = address.subAdminArea.ifEmpty { null }
            adminArea = address.adminArea.ifEmpty { null }
            postalCode = address.postalCode.ifEmpty { null }
            countryCode = address.countryCode.ifEmpty { null }
            countryName = address.countryName.ifEmpty { null }
            address.latitude?.let { latitude = it }
            address.longitude?.let { longitude = it }
        }

        /** Android's geocoder reports an unreachable or busy service as an IOException. */
        fun toError(failure: GeocodeFailure, message: String): Throwable = when (failure) {
            GeocodeFailure.INVALID -> IllegalArgumentException(message)
            GeocodeFailure.BUSY -> IOException("Service not Available: too many requests ($message)")
            GeocodeFailure.REFUSED,
            GeocodeFailure.UNAVAILABLE,
            -> IOException("Service not Available: $message")
        }
    }
}
