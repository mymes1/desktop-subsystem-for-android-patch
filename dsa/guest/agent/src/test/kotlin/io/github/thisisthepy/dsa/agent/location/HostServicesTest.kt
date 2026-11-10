package io.github.thisisthepy.dsa.agent.location

import io.github.thisisthepy.dsa.agent.FakeScheduler
import io.github.thisisthepy.dsa.agent.RecordingLog
import io.github.thisisthepy.dsa.agent.decode
import io.github.thisisthepy.dsa.agent.geocode.GeocodeAnswer
import io.github.thisisthepy.dsa.agent.geocode.GeocodeFailure
import io.github.thisisthepy.dsa.agent.geocode.GeocodeLookup
import io.github.thisisthepy.dsa.agent.geocode.HostAddress
import io.github.thisisthepy.dsa.agent.geocode.HostGeocoder
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.GeocodeAddress
import io.github.thisisthepy.dsa.control.v1.GeocodeResult
import io.github.thisisthepy.dsa.control.v1.GeocodeStatus
import io.github.thisisthepy.dsa.control.v1.LocationAccess
import io.github.thisisthepy.dsa.control.v1.LocationFix
import io.github.thisisthepy.dsa.control.v1.LocationStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The network location provider and the geocoder, both answered by the host. */
class HostServicesTest {
    private val log = RecordingLog()
    private val sent = mutableListOf<Envelope>()
    private var connected = true
    private val scheduler = FakeScheduler()

    private class RecordingSink : NetworkLocationSink {
        val fixes = mutableListOf<HostFix>()
        val allowed = mutableListOf<Boolean>()
        override fun report(fix: HostFix) {
            fixes += fix
        }
        override fun setAllowed(allowed: Boolean) {
            this.allowed += allowed
        }
    }

    private class RecordingAnswer : GeocodeAnswer {
        var found: List<HostAddress>? = null
        var failed: Pair<GeocodeFailure, String>? = null
        override fun found(addresses: List<HostAddress>) {
            check(found == null && failed == null)
            found = addresses
        }
        override fun failed(failure: GeocodeFailure, message: String) {
            check(found == null && failed == null)
            failed = failure to message
        }
    }

    private val location = NetworkLocation({ sent += decode(it); true }, log) { connected }
    private val geocoder = HostGeocoder({ sent += decode(it); true }, log, { connected }, scheduler)

    private fun fix(lat: Double, lon: Double, accuracy: Float = 25f): Envelope =
        Envelope.newBuilder().setLocationFix(
            LocationFix.newBuilder().setLatitudeDegrees(lat).setLongitudeDegrees(lon)
                .setHorizontalAccuracyMeters(accuracy).setUnixTimeMs(1_700_000_000_000L),
        ).build()

    private fun status(access: LocationAccess): Envelope =
        Envelope.newBuilder().setLocationStatus(LocationStatus.newBuilder().setAccess(access)).build()

    private val cityHall = GeocodeAddress.newBuilder()
        .addAddressLines("110 Sejong-daero, Jung-gu, Seoul, South Korea")
        .setFeatureName("Seoul City Hall")
        .setSubThoroughfare("110")
        .setThoroughfare("Sejong-daero")
        .setLocality("Seoul")
        .setPostalCode("04524")
        .setCountryCode("KR")
        .setCountryName("South Korea")
        .setHasPosition(true)
        .setLatitudeDegrees(37.5663)
        .setLongitudeDegrees(126.9779)
        .build()

    @Test
    fun fr36_network_location_and_geocoder_come_from_the_host() {
        // Android wants network locations: the host is asked for its location.
        val sink = RecordingSink()
        location.sink = sink
        location.request(active = true, intervalMillis = 5_000)
        val request = sent.single().locationRequest
        assertTrue(request.active)
        assertEquals(5_000, request.intervalMs)

        // The host's position comes out as the network location, with the same values.
        location.handle(status(LocationAccess.LOCATION_ACCESS_ALLOWED))
        location.handle(fix(37.5, 127.0))
        assertEquals(listOf(true), sink.allowed)
        val reported = sink.fixes.single()
        assertEquals(37.5, reported.latitudeDegrees, 0.0)
        assertEquals(127.0, reported.longitudeDegrees, 0.0)
        assertEquals(25f, reported.accuracyMeters, 0f)
        assertEquals(1_700_000_000_000L, reported.unixTimeMs)

        // The geocoder asks the host, and the host's address comes out as the app's address.
        sent.clear()
        val answer = RecordingAnswer()
        geocoder.lookup(GeocodeLookup.Forward("Seoul City Hall", null, 3, "ko-KR"), answer)
        val asked = sent.single().geocodeRequest
        assertFalse(asked.reverse)
        assertEquals("Seoul City Hall", asked.query)
        assertEquals(3, asked.maxResults)
        assertEquals("ko-KR", asked.language)
        assertTrue(
            geocoder.handle(
                Envelope.newBuilder().setGeocodeResult(
                    GeocodeResult.newBuilder().setRequestId(asked.requestId)
                        .setStatus(GeocodeStatus.GEOCODE_STATUS_OK).addAddresses(cityHall),
                ).build(),
            ),
        )
        assertEquals(
            listOf(
                HostAddress(
                    addressLines = listOf("110 Sejong-daero, Jung-gu, Seoul, South Korea"),
                    featureName = "Seoul City Hall",
                    subThoroughfare = "110",
                    thoroughfare = "Sejong-daero",
                    locality = "Seoul",
                    postalCode = "04524",
                    countryCode = "KR",
                    countryName = "South Korea",
                    latitude = 37.5663,
                    longitude = 126.9779,
                ),
            ),
            answer.found,
        )

        // A reverse lookup carries the position.
        sent.clear()
        geocoder.lookup(GeocodeLookup.Reverse(37.5663, 126.9779, 1, ""), RecordingAnswer())
        val reverse = sent.single().geocodeRequest
        assertTrue(reverse.reverse)
        assertEquals(37.5663, reverse.latitudeDegrees, 0.0)
        assertEquals(126.9779, reverse.longitudeDegrees, 0.0)
    }

    @Test
    fun fr36_the_host_location_reaches_the_network_provider_only_while_android_wants_it() {
        val sink = RecordingSink()
        location.sink = sink
        // The GNSS HAL's fixes reach the agent too; the network provider takes none of them.
        location.handle(fix(1.0, 2.0))
        assertTrue(sink.fixes.isEmpty())
        assertTrue(sent.isEmpty())

        location.request(active = true, intervalMillis = 100)
        assertEquals(NetworkLocation.MIN_INTERVAL_MS.toInt(), sent.single().locationRequest.intervalMs)
        location.request(active = true, intervalMillis = 100)
        assertEquals("the same request is not sent again", 1, sent.size)

        // A position the host gave no accuracy for still has one; one off Earth is dropped.
        location.handle(fix(1.0, 2.0, accuracy = 0f))
        location.handle(fix(100.0, 2.0))
        assertEquals(NetworkLocation.UNKNOWN_ACCURACY_METERS, sink.fixes.single().accuracyMeters, 0f)

        location.request(active = false, intervalMillis = 0)
        assertFalse(sent.last().locationRequest.active)
        location.handle(fix(1.0, 2.0))
        assertEquals(1, sink.fixes.size)
    }

    @Test
    fun fr36_location_off_in_the_hosts_privacy_settings_disables_the_network_provider() {
        val sink = RecordingSink()
        location.sink = sink
        location.request(active = true, intervalMillis = 1_000)
        location.handle(status(LocationAccess.LOCATION_ACCESS_DENIED))
        assertEquals(listOf(false), sink.allowed)
    }

    @Test
    fun fr36_a_reconnected_host_is_asked_again_for_the_location_android_still_wants() {
        connected = false
        location.request(active = true, intervalMillis = 2_000)
        assertTrue(sent.isEmpty())
        connected = true
        location.connected()
        assertEquals(2_000, sent.single().locationRequest.intervalMs)
    }

    @Test
    fun fr36_geocoding_failures_reach_the_app_as_androids_failures() {
        val statuses = mapOf(
            GeocodeStatus.GEOCODE_STATUS_UNAVAILABLE to GeocodeFailure.UNAVAILABLE,
            GeocodeStatus.GEOCODE_STATUS_REFUSED to GeocodeFailure.REFUSED,
            GeocodeStatus.GEOCODE_STATUS_BUSY to GeocodeFailure.BUSY,
            GeocodeStatus.GEOCODE_STATUS_INVALID to GeocodeFailure.INVALID,
        )
        for ((status, failure) in statuses) {
            sent.clear()
            val answer = RecordingAnswer()
            geocoder.lookup(GeocodeLookup.Forward("x", null, 1, ""), answer)
            val id = sent.single().geocodeRequest.requestId
            geocoder.handle(
                Envelope.newBuilder().setGeocodeResult(
                    GeocodeResult.newBuilder().setRequestId(id).setStatus(status).setMessage("why"),
                ).build(),
            )
            assertEquals(failure to "why", answer.failed)
        }

        // No host, or no answer in time, is "service not available".
        connected = false
        val offline = RecordingAnswer()
        geocoder.lookup(GeocodeLookup.Forward("x", null, 1, ""), offline)
        assertEquals(GeocodeFailure.UNAVAILABLE, offline.failed?.first)
        connected = true
        val late = RecordingAnswer()
        geocoder.lookup(GeocodeLookup.Forward("x", null, 1, ""), late)
        scheduler.advance(HostGeocoder.ANSWER_MS)
        assertEquals(GeocodeFailure.UNAVAILABLE, late.failed?.first)
    }
}
