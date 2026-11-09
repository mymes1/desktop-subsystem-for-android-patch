package io.github.thisisthepy.dsa.agent.gapps

import io.github.thisisthepy.dsa.agent.RecordingLog
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class GoogleAppsCompileTest {
    private class FakePort : GoogleAppsCompilePort {
        var onPackage: (String) -> Unit = {}
        var onScreenOff: () -> Unit = {}
        val requests = mutableListOf<String>()
        var failure: Exception? = null

        override fun listen(onPackage: (String) -> Unit, onScreenOff: () -> Unit) {
            this.onPackage = onPackage
            this.onScreenOff = onScreenOff
        }

        override fun request(value: String) {
            failure?.let { throw it }
            requests += value
        }
    }

    private val port = FakePort()
    private val log = RecordingLog()
    private var now = 1000L

    private fun started() = GoogleAppsCompile(port, { now }, log).also { it.start() }

    @Test
    fun nfr2_an_update_of_play_services_or_the_play_store_asks_for_the_compile() {
        started()
        port.onPackage("com.google.android.gms")
        now = 2000
        port.onPackage("com.android.vending")
        assertEquals(listOf("1000", "2000"), port.requests)
        assertEquals("vendor.dsa.gapps.compile", GoogleAppsCompile.PROPERTY)
    }

    @Test
    fun nfr2_an_update_of_any_other_app_does_not() {
        started()
        port.onPackage("com.google.android.youtube")
        port.onPackage("io.github.thisisthepy.dsa.agent")
        assertEquals(emptyList<String>(), port.requests)
    }

    @Test
    fun nfr2_each_screen_off_asks_again_with_a_value_init_has_not_seen() {
        // init acts only when the property changes, and two requests can come within one tick.
        started()
        port.onScreenOff()
        port.onScreenOff()
        port.onPackage("com.google.android.gms")
        assertEquals(listOf("1000", "1001", "1002"), port.requests)
    }

    @Test
    fun nfr2_a_refused_request_is_logged_and_the_next_one_still_goes() {
        started()
        port.failure = RuntimeException("failed to set system property")
        port.onScreenOff()
        assertTrue(log.lines.any { it.startsWith("warn: could not ask for the Google apps") })
        port.failure = null
        port.onScreenOff()
        assertEquals(listOf("1001"), port.requests)
    }
}
