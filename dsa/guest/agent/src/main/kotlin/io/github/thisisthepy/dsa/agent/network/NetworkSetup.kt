package io.github.thisisthepy.dsa.agent.network

import io.github.thisisthepy.dsa.agent.Log

/**
 * The guest's Wi-Fi, as far as joining the host's network needs it. The VM's network adapter
 * appears as a virt_wifi radio that sees one open network, [HOST_NETWORK]; its address,
 * route and DNS servers come by DHCP from the host.
 */
interface WifiPort {
    fun isEnabled(): Boolean

    fun enable()

    /** The SSIDs of the saved networks, without quotes. */
    fun savedNetworks(): List<String>

    /** Saves an open network, so the framework joins it now and whenever it is in range. */
    fun saveOpenNetwork(ssid: String): Boolean

    /** Asks the framework to join the saved network [ssid] now. */
    fun connect(ssid: String)

    companion object {
        val NONE = object : WifiPort {
            override fun isEnabled() = true
            override fun enable() {}
            override fun savedNetworks() = listOf(HOST_NETWORK)
            override fun saveOpenNetwork(ssid: String) = true
            override fun connect(ssid: String) {}
        }
    }
}

/** The SSID the virt_wifi radio reports for the host's network. */
const val HOST_NETWORK = "VirtWifi"

/**
 * Turns Wi-Fi on and keeps the host's network saved, at every start of the agent, so apps
 * have a network from the first boot of a new disk and after a user removed the network in
 * Settings. There is no setup wizard and no SystemUI to do it, and Android leaves Wi-Fi off on
 * a new device. The framework then joins it, validates it and reports it to apps as an
 * unmetered Wi-Fi network; after a host network change or a resume from sleep it renews the
 * DHCP lease on the same network, which stays joined.
 */
class NetworkSetup(private val wifi: WifiPort, private val log: Log) {
    fun joinHostNetwork() {
        if (!wifi.isEnabled()) {
            wifi.enable()
            log.info("turned Wi-Fi on, which carries the host's network")
        }
        if (HOST_NETWORK !in wifi.savedNetworks()) {
            if (wifi.saveOpenNetwork(HOST_NETWORK)) {
                log.info("saved the host's network")
            } else {
                log.warn("could not save the host's network; apps have no network until Wi-Fi joins it")
                return
            }
        }
        wifi.connect(HOST_NETWORK)
    }
}
