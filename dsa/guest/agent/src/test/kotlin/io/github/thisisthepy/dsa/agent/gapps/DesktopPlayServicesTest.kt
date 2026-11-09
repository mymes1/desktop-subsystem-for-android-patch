package io.github.thisisthepy.dsa.agent.gapps

import io.github.thisisthepy.dsa.agent.AGENT
import io.github.thisisthepy.dsa.agent.Agent
import io.github.thisisthepy.dsa.agent.FakeActions
import io.github.thisisthepy.dsa.agent.FakeEndpoint
import io.github.thisisthepy.dsa.agent.FakeInjector
import io.github.thisisthepy.dsa.agent.FakeLocator
import io.github.thisisthepy.dsa.agent.FakeScheduler
import io.github.thisisthepy.dsa.agent.FakeTasks
import io.github.thisisthepy.dsa.agent.FakeWindowing
import io.github.thisisthepy.dsa.agent.RecordingLog
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The package manager as far as component states go. Like the real one, every change of a
 * package's components is announced, the agent's own changes included, and an update of
 * Play services is announced too.
 */
class FakeComponentStates : ComponentStates {
    /** Each installed package's declared components, and whether each is off. */
    val packages = mutableMapOf<String, MutableMap<String, Boolean>>()
    val turnOffCalls = mutableListOf<List<String>>()
    private val listeners = mutableListOf<(String) -> Unit>()

    override fun installed(packageName: String) = packageName in packages

    override fun isOff(packageName: String, className: String): Boolean? = packages[packageName]?.get(className)

    override fun turnOff(packageName: String, classNames: List<String>) {
        val components = packages.getValue(packageName)
        for (name in classNames) require(name in components) { "Component class $name does not exist in $packageName" }
        turnOffCalls += classNames
        for (name in classNames) components[name] = true
        announce(packageName)
    }

    override fun listen(onChange: (String) -> Unit) {
        listeners += onChange
    }

    /** Play services installed, with every component on. */
    fun install(packageName: String, classNames: Collection<String>) {
        packages[packageName] = classNames.associateWith { false }.toMutableMap()
        announce(packageName)
    }

    /** An update that, as Play services' own setup does, turns the components back on. */
    fun update(packageName: String, classNames: Collection<String> = packages.getValue(packageName).keys) {
        packages[packageName] = classNames.associateWith { false }.toMutableMap()
        announce(packageName)
    }

    /** Play services turning one of its own components on, as a module download does. */
    fun turnOn(packageName: String, className: String) {
        packages.getValue(packageName)[className] = false
        announce(packageName)
    }

    fun announce(packageName: String) = listeners.forEach { it(packageName) }

    fun on(packageName: String): Set<String> =
        packages.getValue(packageName).filterValues { off -> !off }.keys
}

class DesktopPlayServicesTest {
    private val scheduler = FakeScheduler()
    private val log = RecordingLog()
    private val states = FakeComponentStates()
    private val gms = DesktopPlayServices.PACKAGE
    private val unused = DesktopPlayServices.UNUSED.map { it.className }

    /** What Play services runs that the agent leaves alone: push, accounts, location, Integrity. */
    private val needed = listOf(
        "com.google.android.gms.gcm.GcmService",
        "com.google.android.gms.chimera.PersistentApiService",
        "com.google.android.location.fused.FusedLocationService",
        "com.google.android.gms.play.integrity.api.service.IntegrityApiPersistentService",
        "com.google.android.gms.droidguard.DroidGuardService",
        "com.google.android.gms.backup.BackupTransportService",
    )

    private fun startedAgent(): Agent {
        val locator = FakeLocator().apply { up = FakeEndpoint() }
        val agent = Agent(
            AGENT, FakeTasks(), FakeActions(), locator, FakeWindowing(), FakeInjector(), scheduler, log,
            inputMethods = {},
            clock = { scheduler.now },
            componentStates = states,
        )
        agent.start()
        scheduler.runDue()
        return agent
    }

    @Test
    fun fr32_desktop_irrelevant_play_services_stay_off_after_updates() {
        states.install(gms, unused + needed)
        startedAgent()

        // At boot: the unused ones are off, the rest are as they were.
        assertEquals(needed.toSet(), states.on(gms))
        assertTrue(log.lines.toString(), log.lines.any { it.startsWith("info: turned off Google Play services components") && "crisis alerts" in it })

        // An update turns them all back on; a moment later they are off again.
        states.update(gms)
        assertEquals((unused + needed).toSet(), states.on(gms))
        scheduler.advance(DesktopPlayServices.SETTLE_MILLIS)
        assertEquals(needed.toSet(), states.on(gms))

        // Play services turns one on by itself, as a module download or a server flag does.
        states.turnOn(gms, unused.first())
        scheduler.advance(DesktopPlayServices.SETTLE_MILLIS)
        assertEquals(needed.toSet(), states.on(gms))
        assertTrue(log.lines.toString(), log.lines.any { it.startsWith("info: turned off again") && "crisis alerts" in it })
    }

    @Test
    fun fr32_play_services_installed_after_boot_has_its_unused_components_turned_off() {
        startedAgent()
        assertTrue(states.turnOffCalls.isEmpty())

        states.install(gms, unused + needed)
        scheduler.advance(DesktopPlayServices.SETTLE_MILLIS)

        assertEquals(needed.toSet(), states.on(gms))
    }

    @Test
    fun fr32_without_google_play_nothing_is_turned_off() {
        states.install("org.example.notes", listOf("org.example.notes.SyncService"))
        startedAgent()
        states.announce("org.example.notes")
        scheduler.advance(DesktopPlayServices.SETTLE_MILLIS)

        assertTrue(states.turnOffCalls.isEmpty())
        assertFalse(log.lines.any { "Google Play services" in it })
    }

    @Test
    fun fr32_a_component_a_play_services_version_does_not_have_is_left_out() {
        // Play services without the Cast suggestions service, which the package manager would
        // refuse to change, failing the whole batch.
        val missing = "com.google.android.gms.cast.devicesuggestions.SuggestionsProviderChimeraService"
        states.install(gms, unused - missing + needed)
        startedAgent()

        assertEquals(needed.toSet(), states.on(gms))
        assertFalse(states.turnOffCalls.single().contains(missing))
    }

    @Test
    fun fr32_a_burst_of_changes_is_read_once_and_the_agents_own_change_is_no_work() {
        states.install(gms, unused + needed)
        startedAgent()
        assertEquals(1, states.turnOffCalls.size)

        repeat(10) { states.update(gms) }
        scheduler.advance(DesktopPlayServices.SETTLE_MILLIS * 3)

        // One turn-off for the burst; the change it announced finds everything off.
        assertEquals(2, states.turnOffCalls.size)
    }

    @Test
    fun fr32_a_component_play_services_keeps_turning_on_is_left_after_a_while() {
        states.install(gms, unused + needed)
        startedAgent()
        val first = unused.first()

        repeat(DesktopPlayServices.MAX_REAPPLY + 2) {
            states.turnOn(gms, first)
            scheduler.advance(DesktopPlayServices.SETTLE_MILLIS)
        }

        assertEquals(setOf(first) + needed, states.on(gms))
        assertEquals(1 + DesktopPlayServices.MAX_REAPPLY, states.turnOffCalls.size)
        assertTrue(log.lines.toString(), log.lines.any { it.startsWith("warn: Google Play services turned crisis alerts") })

        // The others are still put back after an update.
        states.update(gms)
        scheduler.advance(DesktopPlayServices.SETTLE_MILLIS)
        assertEquals(setOf(first) + needed, states.on(gms))
    }

    @Test
    fun fr32_a_refused_change_is_logged_and_the_agent_runs_on() {
        val refusing = object : ComponentStates by states {
            override fun turnOff(packageName: String, classNames: List<String>) = throw SecurityException("no permission")
        }
        states.install(gms, unused + needed)
        val play = DesktopPlayServices(refusing, scheduler, log, clock = { scheduler.now })
        play.start()

        assertTrue(log.lines.toString(), log.lines.any { it.startsWith("warn: could not turn off Google Play services components") })
    }
}
