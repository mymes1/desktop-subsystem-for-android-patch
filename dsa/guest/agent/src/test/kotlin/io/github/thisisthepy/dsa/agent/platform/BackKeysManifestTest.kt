package io.github.thisisthepy.dsa.agent.platform

import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element

/**
 * The agent hears the Escapes no app takes from the framework's key gesture events, which
 * take MANAGE_KEY_GESTURES. The agent's manifest asks for it, and its privileged
 * permission list names it with the rest.
 */
class BackKeysManifestTest {
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
    fun fr5_the_agent_may_hear_the_escapes_no_app_takes() {
        val permission = "android.permission.MANAGE_KEY_GESTURES"
        assertTrue(permission in names("src/main/AndroidManifest.xml", "uses-permission"))
        assertTrue(permission in names("privapp-permissions-dsa-agent.xml", "permission"))
    }
}
