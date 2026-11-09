package io.github.thisisthepy.dsa.agent.platform

import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element

/** The backup transport and the passkey provider, as the manifest declares them. */
class HostServicesManifestTest {
    private val android = "http://schemas.android.com/apk/res/android"

    private val manifest: Element = DocumentBuilderFactory.newInstance()
        .apply { isNamespaceAware = true }
        .newDocumentBuilder()
        .parse(File("src/main/AndroidManifest.xml"))
        .documentElement

    private fun Element.children(tag: String): List<Element> {
        val nodes = getElementsByTagName(tag)
        return (0 until nodes.length).map { nodes.item(it) as Element }
    }

    private fun Element.attr(name: String): String = getAttributeNS(android, name)

    private fun service(action: String): Element? = manifest.children("service").singleOrNull { service ->
        service.children("action").any { it.attr("name") == action }
    }

    private val permissions = manifest.children("uses-permission").map { it.attr("name") }.toSet()

    @Test
    fun fr36_backup_transport_is_bound_only_by_the_system() {
        val service = service("android.backup.TRANSPORT_HOST")
        assertNotNull("the agent declares no backup transport", service)
        assertEquals(".platform.HostBackupTransportService", service!!.attr("name"))
        assertEquals("false", service.attr("exported"))
        assertTrue("android.permission.BACKUP" in permissions)
        // The backup manager uses the transport only because the image's sysconfig lists it.
        val listed = File("sysconfig-dsa-agent.xml").readText()
        assertTrue(listed.contains(
            "service=\"io.github.thisisthepy.dsa.agent/io.github.thisisthepy.dsa.agent.platform.HostBackupTransportService\"",
        ))
    }

    @Test
    fun fr36_passkey_provider_is_bound_only_by_the_credential_manager() {
        val service = service("android.service.credentials.CredentialProviderService")
        assertNotNull("the agent declares no credential provider", service)
        assertEquals(".platform.HostCredentialProvider", service!!.attr("name"))
        assertEquals("android.permission.BIND_CREDENTIAL_PROVIDER_SERVICE", service.attr("permission"))
        assertEquals(service, service("android.service.credentials.system.CredentialProviderService"))
        val metaData = service.children("meta-data").single()
        assertEquals("android.credentials.provider", metaData.attr("name"))
        assertEquals("@xml/host_credential_provider", metaData.attr("resource"))
        val capabilities = File("src/main/res/xml/host_credential_provider.xml").readText()
        assertTrue(capabilities.contains("<capability name=\"androidx.credentials.TYPE_PUBLIC_KEY_CREDENTIAL\" />"))
        assertTrue("android.permission.PROVIDE_DEFAULT_ENABLED_CREDENTIAL_SERVICE" in permissions)
        // The activity the provider's entries open is the agent's alone.
        val activity = manifest.children("activity").single { it.attr("name") == ".platform.PasskeyActivity" }
        assertEquals("false", activity.attr("exported"))
    }
}
