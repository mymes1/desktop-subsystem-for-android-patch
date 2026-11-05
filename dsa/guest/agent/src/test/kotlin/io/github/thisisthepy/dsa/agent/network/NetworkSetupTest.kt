package io.github.thisisthepy.dsa.agent.network

import io.github.thisisthepy.dsa.agent.Log
import org.junit.Test
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue

private class FakeWifi(var enabled: Boolean = false, val saved: MutableList<String> = mutableListOf()) : WifiPort {
    var saveWorks = true
    val connected = mutableListOf<String>()

    override fun isEnabled() = enabled

    override fun enable() {
        enabled = true
    }

    override fun savedNetworks(): List<String> = saved

    override fun saveOpenNetwork(ssid: String): Boolean {
        if (saveWorks) saved += ssid
        return saveWorks
    }

    override fun connect(ssid: String) {
        connected += ssid
    }
}

private class Lines : Log {
    val lines = mutableListOf<String>()

    override fun info(message: String) {
        lines += message
    }

    override fun warn(message: String, error: Throwable?) {
        lines += "warn: $message"
    }
}

class NetworkSetupTest {
    @Test
    fun fr28_a_new_disk_turns_wifi_on_and_joins_the_host_network() {
        val wifi = FakeWifi()
        NetworkSetup(wifi, Lines()).joinHostNetwork()
        assertTrue(wifi.enabled)
        assertEquals(listOf(HOST_NETWORK), wifi.saved)
        assertEquals(listOf(HOST_NETWORK), wifi.connected)
    }

    @Test
    fun fr28_a_saved_network_is_not_saved_twice() {
        val wifi = FakeWifi(enabled = true, saved = mutableListOf(HOST_NETWORK))
        val log = Lines()
        NetworkSetup(wifi, log).joinHostNetwork()
        assertEquals(listOf(HOST_NETWORK), wifi.saved)
        assertEquals(listOf(HOST_NETWORK), wifi.connected)
        assertTrue(log.lines.isEmpty())
    }

    @Test
    fun fr28_a_network_that_cannot_be_saved_is_logged_not_joined() {
        val wifi = FakeWifi().apply { saveWorks = false }
        val log = Lines()
        NetworkSetup(wifi, log).joinHostNetwork()
        assertTrue(wifi.connected.isEmpty())
        assertTrue(log.lines.any { it.startsWith("warn:") })
    }
}
