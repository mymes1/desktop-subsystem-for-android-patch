package io.github.thisisthepy.dsa.agent.platform

import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element

/**
 * The agent makes the host's time zone, languages, dark theme and accent color the guest's,
 * which takes SET_TIME_ZONE, CHANGE_CONFIGURATION, MODIFY_DAY_NIGHT_MODE and
 * CHANGE_OVERLAY_PACKAGES. The agent's manifest asks for them, and its privileged permission
 * list names them with the rest.
 */
class SystemSettingsManifestTest {
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
    fun fr30_the_agent_may_change_the_guests_system_settings() {
        val manifest = names("src/main/AndroidManifest.xml", "uses-permission")
        val privileged = names("privapp-permissions-dsa-agent.xml", "permission")
        for (permission in listOf(
            "android.permission.SET_TIME_ZONE",
            "android.permission.CHANGE_CONFIGURATION",
            "android.permission.MODIFY_DAY_NIGHT_MODE",
            "android.permission.CHANGE_OVERLAY_PACKAGES",
        )) {
            assertTrue(permission, permission in manifest)
            assertTrue(permission, permission in privileged)
        }
    }
}
