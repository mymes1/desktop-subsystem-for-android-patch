package io.github.thisisthepy.dsa.agent.geocode

import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.agent.Scheduler
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.GeocodeAddress
import io.github.thisisthepy.dsa.control.v1.GeocodeRequest
import io.github.thisisthepy.dsa.control.v1.GeocodeResult
import io.github.thisisthepy.dsa.control.v1.GeocodeStatus

/** The box an app prefers forward results in, in degrees. */
data class GeoBounds(
    val lowerLeftLatitude: Double,
    val lowerLeftLongitude: Double,
    val upperRightLatitude: Double,
    val upperRightLongitude: Double,
)

/** One lookup an app asked Android's geocoder for. */
sealed class GeocodeLookup {
    abstract val maxResults: Int

    /** A BCP 47 tag ("ko-KR"), or empty for the host's own language. */
    abstract val language: String

    data class Forward(
        val query: String,
        val bounds: GeoBounds?,
        override val maxResults: Int,
        override val language: String,
    ) : GeocodeLookup()

    data class Reverse(
        val latitude: Double,
        val longitude: Double,
        override val maxResults: Int,
        override val language: String,
    ) : GeocodeLookup()
}

/** One address the host found. Empty strings are parts the host did not give. */
data class HostAddress(
    val addressLines: List<String>,
    val featureName: String = "",
    val premises: String = "",
    val subThoroughfare: String = "",
    val thoroughfare: String = "",
    val subLocality: String = "",
    val locality: String = "",
    val subAdminArea: String = "",
    val adminArea: String = "",
    val postalCode: String = "",
    val countryCode: String = "",
    val countryName: String = "",
    val latitude: Double? = null,
    val longitude: Double? = null,
)

/** Why a lookup has no answer, as Android's geocoder reports it to the app. */
enum class GeocodeFailure {
    /** No network, or the host's geocoding service failed: Android's "service not available". */
    UNAVAILABLE,

    /** The host does not look addresses up now (turned off, or no desktop session). */
    REFUSED,

    /** Too many lookups in a short time. */
    BUSY,

    /** The lookup cannot be answered as it is: a bad argument in Android's terms. */
    INVALID,
}

/** Where a lookup's answer goes: exactly one of the two is called, once. */
interface GeocodeAnswer {
    fun found(addresses: List<HostAddress>)

    fun failed(failure: GeocodeFailure, message: String)
}

/**
 * The guest's geocoder, answered by the host.
 *
 * The agent is the guest's geocode provider. Each lookup an app makes through Android's
 * `Geocoder` comes here ([lookup]) and goes to the host as a `GeocodeRequest`; the host looks
 * it up with its own geocoding service and answers with a `GeocodeResult`, whose addresses go
 * back to the app, or whose failure becomes the error Android's geocoder reports. A lookup the
 * host does not answer within [ANSWER_MS] fails as unavailable, as does one made while the
 * host cannot be reached.
 *
 * Everything runs on the agent's thread.
 */
class HostGeocoder(
    private val send: (ByteArray) -> Boolean,
    private val log: Log,
    private val isConnected: () -> Boolean,
    private val scheduler: Scheduler,
) {
    private val pending = mutableMapOf<Int, GeocodeAnswer>()
    private var lastId = 0

    /** Looks [lookup] up through the host, answering [answer]. */
    fun lookup(lookup: GeocodeLookup, answer: GeocodeAnswer) {
        if (!isConnected()) {
            answer.failed(GeocodeFailure.UNAVAILABLE, "the PC cannot be reached to look the address up")
            return
        }
        if (pending.size >= MAX_PENDING) {
            answer.failed(GeocodeFailure.BUSY, "too many address lookups are waiting for the PC")
            return
        }
        lastId = if (lastId == Int.MAX_VALUE) 1 else lastId + 1
        val id = lastId
        val request = GeocodeRequest.newBuilder()
            .setRequestId(id)
            .setMaxResults(lookup.maxResults.coerceIn(1, MAX_RESULTS))
            .setLanguage(lookup.language)
        when (lookup) {
            is GeocodeLookup.Forward -> {
                request.setQuery(lookup.query)
                lookup.bounds?.let {
                    request.setHasBounds(true)
                        .setLowerLeftLatitude(it.lowerLeftLatitude)
                        .setLowerLeftLongitude(it.lowerLeftLongitude)
                        .setUpperRightLatitude(it.upperRightLatitude)
                        .setUpperRightLongitude(it.upperRightLongitude)
                }
            }
            is GeocodeLookup.Reverse -> request.setReverse(true)
                .setLatitudeDegrees(lookup.latitude)
                .setLongitudeDegrees(lookup.longitude)
        }
        pending[id] = answer
        if (!send(Envelope.newBuilder().setGeocodeRequest(request).build().toByteArray())) {
            pending.remove(id)
            answer.failed(GeocodeFailure.UNAVAILABLE, "the lookup could not be sent to the PC")
            return
        }
        scheduler.postDelayed(ANSWER_MS) {
            pending.remove(id)?.failed(GeocodeFailure.UNAVAILABLE, "the PC did not answer the lookup in time")
        }
    }

    /** Takes a `GeocodeResult`. False for any other frame. */
    fun handle(envelope: Envelope): Boolean {
        if (envelope.bodyCase != Envelope.BodyCase.GEOCODE_RESULT) return false
        val result = envelope.geocodeResult
        val answer = pending.remove(result.requestId)
        if (answer == null) {
            log.info("geocoder: dropped the PC's answer to lookup ${result.requestId}, which nobody waits for")
            return true
        }
        answer(result, answer)
        return true
    }

    private fun answer(result: GeocodeResult, answer: GeocodeAnswer) {
        when (result.status) {
            GeocodeStatus.GEOCODE_STATUS_OK -> answer.found(result.addressesList.map(::toAddress))
            GeocodeStatus.GEOCODE_STATUS_BUSY -> answer.failed(GeocodeFailure.BUSY, message(result, "too many lookups"))
            GeocodeStatus.GEOCODE_STATUS_REFUSED ->
                answer.failed(GeocodeFailure.REFUSED, message(result, "the PC does not look addresses up now"))
            GeocodeStatus.GEOCODE_STATUS_INVALID ->
                answer.failed(GeocodeFailure.INVALID, message(result, "the lookup cannot be answered"))
            else -> answer.failed(GeocodeFailure.UNAVAILABLE, message(result, "the PC's geocoding service is not available"))
        }
    }

    private fun message(result: GeocodeResult, fallback: String): String =
        result.message.ifBlank { fallback }

    companion object {
        /** The most addresses one lookup asks for. */
        const val MAX_RESULTS = 10

        /** The most lookups waiting for the host at once. */
        const val MAX_PENDING = 32

        /** How long a lookup waits for the host's answer. */
        const val ANSWER_MS = 30_000L

        fun toAddress(address: GeocodeAddress): HostAddress = HostAddress(
            addressLines = address.addressLinesList.toList(),
            featureName = address.featureName,
            premises = address.premises,
            subThoroughfare = address.subThoroughfare,
            thoroughfare = address.thoroughfare,
            subLocality = address.subLocality,
            locality = address.locality,
            subAdminArea = address.subAdminArea,
            adminArea = address.adminArea,
            postalCode = address.postalCode,
            countryCode = address.countryCode,
            countryName = address.countryName,
            latitude = if (address.hasPosition) address.latitudeDegrees else null,
            longitude = if (address.hasPosition) address.longitudeDegrees else null,
        )
    }
}
