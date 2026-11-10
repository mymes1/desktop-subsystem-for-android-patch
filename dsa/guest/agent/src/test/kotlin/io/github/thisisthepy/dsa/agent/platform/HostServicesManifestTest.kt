package io.github.thisisthepy.dsa.agent.platform

import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element

/**
 * The agent is the guest's network location provider and geocode provider, which only the
 * system binds, and "Share on PC" in the share sheet.
 */
class HostServicesManifestTest {
    private val android = "http://schemas.android.com/apk/res/android"

    private val manifest: Element = DocumentBuilderFactory.newInstance()
        .apply { isNamespaceAware = true }
        .newDocumentBuilder()
        .parse(File("src/main/AndroidManifest.xml"))
        .documentElement

    private val privileged: String = File("privapp-permissions-dsa-agent.xml").readText()

    private fun Element.children(tag: String): List<Element> {
        val nodes = getElementsByTagName(tag)
        return (0 until nodes.length).map { nodes.item(it) as Element }
    }

    private fun Element.attr(name: String): String = getAttributeNS(android, name)

    private fun component(tag: String, action: String): Element? =
        manifest.children(tag).singleOrNull { component ->
            component.children("action").any { it.attr("name") == action }
        }

    @Test
    fun fr36_the_network_location_and_geocode_providers_are_bound_by_the_system_only() {
        for ((action, name) in listOf(
            "com.android.location.service.v3.NetworkLocationProvider" to ".platform.HostNetworkLocationService",
            "com.android.location.service.GeocodeProvider" to ".platform.HostGeocodeService",
        )) {
            val service = component("service", action)
            assertNotNull("the agent declares no service for $action", service)
            assertEquals(name, service!!.attr("name"))
            assertEquals("android.permission.WRITE_SECURE_SETTINGS", service.attr("permission"))
            assertEquals("", service.attr("process"))
        }
        val held = manifest.children("uses-permission").map { it.attr("name") }
        assertTrue("android.permission.INSTALL_LOCATION_PROVIDER" in held)
        assertTrue(privileged.contains("android.permission.INSTALL_LOCATION_PROVIDER"))
    }

    @Test
    fun fr36_share_on_pc_takes_every_kind_of_share() {
        val activity = component("activity", "android.intent.action.SEND")
        assertNotNull("the agent has no share target", activity)
        assertEquals(".platform.ShareActivity", activity!!.attr("name"))
        assertEquals("@string/share_target_name", activity.attr("label"))
        assertEquals("singleInstance", activity.attr("launchMode"))
        val actions = activity.children("action").map { it.attr("name") }
        assertTrue("android.intent.action.SEND_MULTIPLE" in actions)
        assertEquals(listOf("*/*"), activity.children("data").map { it.attr("mimeType") })
    }
}
