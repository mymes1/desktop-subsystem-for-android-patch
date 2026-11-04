package io.github.thisisthepy.dsa.agent.install

import io.github.thisisthepy.dsa.agent.AGENT
import io.github.thisisthepy.dsa.agent.Agent
import io.github.thisisthepy.dsa.agent.FakeActions
import io.github.thisisthepy.dsa.agent.FakeEndpoint
import io.github.thisisthepy.dsa.agent.FakeInjector
import io.github.thisisthepy.dsa.agent.FakeLauncherApps
import io.github.thisisthepy.dsa.agent.FakeLocator
import io.github.thisisthepy.dsa.agent.FakeScheduler
import io.github.thisisthepy.dsa.agent.FakeTasks
import io.github.thisisthepy.dsa.agent.FakeWindowing
import io.github.thisisthepy.dsa.agent.RecordingLog
import io.github.thisisthepy.dsa.agent.tasks.LauncherApp
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.UninstallApp
import io.github.thisisthepy.dsa.control.v1.UninstallResult
import io.github.thisisthepy.dsa.control.v1.UninstallStatus
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AppUninstallerTest {
    private val scheduler = FakeScheduler()
    private val locator = FakeLocator()
    private val daemon = FakeEndpoint()
    private val log = RecordingLog()
    private val apps = FakeLauncherApps()

    /** The guest's installer: what it holds, and what it answers each uninstall with. */
    private inner class FakeUninstaller : PackageUninstallerPort {
        val kinds = mutableMapOf(
            "com.example.notes" to AppKind.USER,
            "com.example.kept" to AppKind.USER,
            "com.android.settings" to AppKind.SYSTEM,
        )
        val uninstalled = mutableListOf<String>()
        var answer = PackageInstallerStatus.SUCCESS

        override fun kind(packageName: String): AppKind? = kinds[packageName]

        override fun uninstall(packageName: String, onResult: (InstallerResult) -> Unit) {
            uninstalled += packageName
            if (answer == PackageInstallerStatus.SUCCESS) {
                kinds.remove(packageName)
                apps.installed = apps.installed.filter { it.packageName != packageName }
            }
            onResult(InstallerResult(answer, if (answer == PackageInstallerStatus.SUCCESS) "Success" else "DELETE_FAILED_DEVICE_POLICY_MANAGER", packageName))
        }
    }

    private val uninstaller = FakeUninstaller()

    private fun started(): Agent {
        apps.installed = listOf(
            LauncherApp("com.android.settings", "Settings", byteArrayOf(1), systemApp = true),
            LauncherApp("com.example.kept", "Kept", byteArrayOf(2)),
            LauncherApp("com.example.notes", "Notes", byteArrayOf(3)),
        )
        locator.up = daemon
        val agent = Agent(
            AGENT, FakeTasks(), FakeActions(), locator, FakeWindowing(), FakeInjector(), scheduler, log,
            launcherApps = apps,
            packageUninstaller = uninstaller,
        )
        agent.start()
        scheduler.runDue()
        return agent
    }

    private fun uninstall(id: Int, name: String) {
        daemon.deliver(
            Envelope.newBuilder().setUninstallApp(UninstallApp.newBuilder().setRequestId(id).setPackageName(name))
                .build().toByteArray(),
        )
        scheduler.runDue()
    }

    private fun results(): List<UninstallResult> = daemon.sentMessages().filter { it.hasUninstallResult() }.map { it.uninstallResult }

    @Test
    fun fr22_a_user_app_is_uninstalled_and_the_host_hears_it_is_gone() {
        started()
        daemon.sent.clear()

        uninstall(7, "com.example.notes")

        assertEquals(listOf("com.example.notes"), uninstaller.uninstalled)
        val result = results().single()
        assertEquals(7, result.requestId)
        assertEquals("com.example.notes", result.packageName)
        assertEquals(UninstallStatus.UNINSTALL_STATUS_SUCCESS, result.status)
        val list = daemon.sentMessages().last { it.hasInstalledApps() }.installedApps.packageNamesList
        assertEquals(listOf("com.android.settings", "com.example.kept"), list)
    }

    @Test
    fun fr22_a_system_app_or_a_missing_package_is_refused_with_its_reason() {
        started()

        uninstall(1, "com.android.settings")
        uninstall(2, "com.example.missing")
        uninstall(3, AGENT)

        assertTrue(uninstaller.uninstalled.isEmpty())
        assertEquals(
            listOf(
                UninstallStatus.UNINSTALL_STATUS_SYSTEM_APP,
                UninstallStatus.UNINSTALL_STATUS_NOT_INSTALLED,
                UninstallStatus.UNINSTALL_STATUS_SYSTEM_APP,
            ),
            results().map { it.status },
        )
        assertEquals(listOf(1, 2, 3), results().map { it.requestId })
        assertTrue(results().all { it.message.isNotEmpty() })
    }

    @Test
    fun fr22_an_uninstall_android_refuses_is_answered_with_its_reason() {
        started()
        uninstaller.answer = PackageInstallerStatus.FAILURE_BLOCKED

        uninstall(4, "com.example.kept")

        val result = results().single()
        assertEquals(UninstallStatus.UNINSTALL_STATUS_BLOCKED, result.status)
        assertEquals("DELETE_FAILED_DEVICE_POLICY_MANAGER", result.message)
    }
}
