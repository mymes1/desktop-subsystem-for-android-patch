package io.github.thisisthepy.dsa.agent.platform

import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element

/**
 * The agent turns Wi-Fi on and saves the host's network for every app, which takes
 * NETWORK_SETTINGS besides the Wi-Fi state permissions. The framework refuses to boot when a
 * privileged app asks for a privileged permission its list does not grant.
 */
class NetworkManifestTest {
    private fun names(path: String, tag: String): Set<String> {
        val document = DocumentBuilderFactory.newInstance()
            .apply { isNamespaceAware = true }
            .newDocumentBuilder()
            .parse(File(path))
        val nodes = document.getElementsByTagName(tag)
        return (0 until nodes.length).map { (nodes.item(it) as Element) }
            .map { element ->
                element.getAttributeNS("http://schemas.android.com/apk/res/android", "name")
                    .ifEmpty { element.getAttribute("name") }
            }
            .toSet()
    }

    @Test
    fun fr28_the_agent_may_turn_wifi_on_and_save_the_host_network() {
        for (permission in listOf(
            "android.permission.ACCESS_WIFI_STATE",
            "android.permission.CHANGE_WIFI_STATE",
            "android.permission.NETWORK_SETTINGS",
        )) {
            assertTrue(permission, permission in names("src/main/AndroidManifest.xml", "uses-permission"))
            assertTrue(permission, permission in names("privapp-permissions-dsa-agent.xml", "permission"))
        }
    }
}
