package io.github.thisisthepy.dsa.agent.platform

import android.content.Context
import android.net.wifi.WifiConfiguration
import android.net.wifi.WifiManager
import io.github.thisisthepy.dsa.agent.network.WifiPort

/**
 * The guest's Wi-Fi through WifiManager. Turning Wi-Fi on and saving a network for the device
 * are the calls Settings makes; the agent may make them because it holds NETWORK_SETTINGS as
 * a platform app. The deprecated WifiConfiguration calls are the ones that save a network
 * for every app rather than suggest one for the caller.
 */
@Suppress("DEPRECATION")
class AndroidWifi(context: Context) : WifiPort {
    private val wifi = context.getSystemService(WifiManager::class.java)

    override fun isEnabled(): Boolean = wifi.isWifiEnabled

    override fun enable() {
        wifi.setWifiEnabled(true)
    }

    override fun savedNetworks(): List<String> =
        wifi.configuredNetworks.orEmpty().mapNotNull { it.SSID?.removeSurrounding("\"") }

    override fun saveOpenNetwork(ssid: String): Boolean {
        val config = WifiConfiguration().apply {
            SSID = "\"$ssid\""
            setSecurityParams(WifiConfiguration.SECURITY_TYPE_OPEN)
        }
        return wifi.addNetwork(config) != -1
    }

    override fun connect(ssid: String) {
        val id = wifi.configuredNetworks.orEmpty()
            .firstOrNull { it.SSID?.removeSurrounding("\"") == ssid }?.networkId ?: return
        wifi.enableNetwork(id, true)
    }
}
