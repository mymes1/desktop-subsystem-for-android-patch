package io.github.thisisthepy.dsa.agent.background

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
import io.github.thisisthepy.dsa.agent.notifications.FakeNotifications
import io.github.thisisthepy.dsa.agent.notifications.notice
import io.github.thisisthepy.dsa.control.v1.BackgroundAppAction
import io.github.thisisthepy.dsa.control.v1.BackgroundAppCommand
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.MediaCommand
import io.github.thisisthepy.dsa.control.v1.MediaControl
import io.github.thisisthepy.dsa.control.v1.MediaPlayback
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The guest's media sessions, as a test sets them. */
class FakeMedia : MediaSource {
    val sessions = linkedMapOf<String, MediaState>()
    val commands = mutableListOf<Pair<String, MediaCommand>>()
    var listener: MediaListener? = null

    override fun listen(listener: MediaListener) {
        this.listener = listener
    }

    override fun active() = sessions.values.toList()

    override fun control(packageName: String, command: MediaCommand): Boolean {
        if (packageName !in sessions) return false
        commands += packageName to command
        return true
    }

    fun change(state: MediaState) {
        sessions[state.packageName] = state
        listener?.changed(state)
    }

    fun end(packageName: String) {
        sessions.remove(packageName)
        listener?.gone(packageName)
    }
}

const val MUSIC = "com.example.music"
const val PLAYER = "0|com.example.music|7|null|10100"

fun playing(title: String = "Song") = MediaState(
    packageName = MUSIC,
    appLabel = "Music",
    title = title,
    artist = "Artist",
    album = "Album",
    artPng = byteArrayOf(0x89.toByte(), 'P'.code.toByte()),
    playback = Playback.PLAYING,
    canPlay = false,
    canPause = true,
    canNext = true,
    canPrevious = true,
)

fun service(key: String = PLAYER, title: String = "Playing", packageName: String = MUSIC) = notice(
    key = key,
    packageName = packageName,
    title = title,
    text = "Song - Artist",
    ongoing = true,
    foregroundService = true,
    clearable = false,
    actions = listOf("Pause", "Next"),
    appLabel = "Music",
)

class BackgroundAppsTest {
    private val scheduler = FakeScheduler()
    private val locator = FakeLocator()
    private val daemon = FakeEndpoint()
    private val log = RecordingLog()
    private val notifications = FakeNotifications()
    private val media = FakeMedia()
    private val stopped = mutableListOf<String>()

    private fun started(connected: Boolean = true): Agent {
        if (connected) locator.up = daemon
        val agent = Agent(
            AGENT, FakeTasks(), FakeActions(), locator, FakeWindowing(), FakeInjector(), scheduler, log,
            notifications = notifications,
            media = media,
            stopper = AppStopper { stopped += it; true },
        )
        agent.start()
        scheduler.runDue()
        return agent
    }

    private fun sentApps() = daemon.sentMessages().filter { it.hasBackgroundApp() }.map { it.backgroundApp }

    private fun sentGone() = daemon.sentMessages().filter { it.hasBackgroundAppGone() }.map { it.backgroundAppGone.packageName }

    private fun sentMedia() = daemon.sentMessages().filter { it.hasMediaSession() }.map { it.mediaSession }

    private fun sentMediaGone() =
        daemon.sentMessages().filter { it.hasMediaSessionGone() }.map { it.mediaSessionGone.packageName }

    private fun fromHost(envelope: Envelope) {
        daemon.deliver(envelope.toByteArray())
        scheduler.runDue()
    }

    private fun action(command: BackgroundAppCommand, packageName: String = MUSIC, index: Int = 0) =
        Envelope.newBuilder().setBackgroundAppAction(
            BackgroundAppAction.newBuilder().setPackageName(packageName).setCommand(command).setAction(index),
        ).build()

    @Test
    fun fr24_a_foreground_service_reaches_the_host_and_its_end_removes_it() {
        started()
        notifications.post(service())
        scheduler.runDue()
        val sent = sentApps().single()
        assertEquals(MUSIC, sent.packageName)
        assertEquals("Music", sent.appLabel)
        assertEquals("Playing", sent.title)
        assertEquals("Song - Artist", sent.text)
        assertEquals(listOf("Pause", "Next"), sent.actionsList)
        assertEquals(4, sent.iconPng.size())

        notifications.remove(PLAYER)
        scheduler.runDue()
        assertEquals(listOf(MUSIC), sentGone())
    }

    @Test
    fun fr24_an_ongoing_service_notification_is_not_a_toast() {
        started()
        notifications.post(service())
        scheduler.runDue()
        assertTrue(daemon.sentMessages().none { it.hasNotification() })
        assertEquals(1, sentApps().size)
    }

    @Test
    fun fr24_one_entry_per_package_gone_only_with_its_last_service() {
        started()
        notifications.post(service(key = "a", title = "Downloading"))
        notifications.post(service(key = "b", title = "Playing"))
        // The same again changes nothing.
        notifications.post(service(key = "b", title = "Playing"))
        notifications.remove("b")
        scheduler.runDue()
        assertEquals(listOf("Downloading", "Playing", "Downloading"), sentApps().map { it.title })
        assertTrue(sentGone().isEmpty())
        notifications.remove("a")
        scheduler.runDue()
        assertEquals(listOf(MUSIC), sentGone())
    }

    @Test
    fun fr24_the_agents_own_and_plain_notifications_are_not_background_apps() {
        started()
        notifications.post(service(packageName = AGENT, key = "own"))
        notifications.post(notice())
        scheduler.runDue()
        assertTrue(sentApps().isEmpty())
    }

    @Test
    fun fr24_on_connecting_the_host_hears_of_every_app_and_session() {
        notifications.shown[PLAYER] = service()
        media.sessions[MUSIC] = playing()
        started()
        assertEquals(listOf(MUSIC), sentApps().map { it.packageName })
        assertEquals(listOf("Song"), sentMedia().map { it.title })
    }

    @Test
    fun fr24_open_and_an_action_run_the_notifications_intents() {
        started()
        notifications.post(service())
        scheduler.runDue()
        fromHost(action(BackgroundAppCommand.BACKGROUND_APP_COMMAND_OPEN))
        fromHost(action(BackgroundAppCommand.BACKGROUND_APP_COMMAND_ACTION, index = 1))
        assertEquals(listOf(PLAYER), notifications.opened)
        assertEquals(listOf(PLAYER to 1), notifications.actionsRun)
    }

    @Test
    fun fr24_stop_force_stops_the_package_and_removes_it_and_its_session_on_the_host() {
        started()
        notifications.post(service())
        media.change(playing())
        scheduler.runDue()
        fromHost(action(BackgroundAppCommand.BACKGROUND_APP_COMMAND_STOP))
        assertEquals(listOf(MUSIC), stopped)
        assertEquals(listOf(MUSIC), sentGone())
        assertEquals(listOf(MUSIC), sentMediaGone())
    }

    @Test
    fun fr24_the_agent_and_the_system_are_never_stopped() {
        started()
        fromHost(action(BackgroundAppCommand.BACKGROUND_APP_COMMAND_STOP, packageName = AGENT))
        fromHost(action(BackgroundAppCommand.BACKGROUND_APP_COMMAND_STOP, packageName = "android"))
        assertTrue(stopped.isEmpty())
    }

    @Test
    fun fr24_acting_on_an_app_that_runs_nothing_removes_it_on_the_host() {
        started()
        fromHost(action(BackgroundAppCommand.BACKGROUND_APP_COMMAND_OPEN))
        assertTrue(notifications.opened.isEmpty())
        assertEquals(listOf(MUSIC), sentGone())
    }

    @Test
    fun fr24_a_media_session_reaches_the_host_and_media_keys_reach_it() {
        started()
        media.change(playing())
        media.change(playing())
        media.change(playing(title = "Next song"))
        scheduler.runDue()
        val sent = sentMedia()
        assertEquals(listOf("Song", "Next song"), sent.map { it.title })
        assertEquals(MediaPlayback.MEDIA_PLAYBACK_PLAYING, sent.last().playback)
        assertTrue(sent.last().canPause)
        assertTrue(sent.last().canNext)

        fromHost(
            Envelope.newBuilder().setMediaControl(
                MediaControl.newBuilder().setPackageName(MUSIC).setCommand(MediaCommand.MEDIA_COMMAND_PAUSE),
            ).build(),
        )
        assertEquals(listOf(MUSIC to MediaCommand.MEDIA_COMMAND_PAUSE), media.commands)

        media.end(MUSIC)
        scheduler.runDue()
        assertEquals(listOf(MUSIC), sentMediaGone())
    }

    @Test
    fun fr24_a_media_key_for_a_session_that_is_gone_removes_it_on_the_host() {
        started()
        fromHost(
            Envelope.newBuilder().setMediaControl(
                MediaControl.newBuilder().setPackageName(MUSIC).setCommand(MediaCommand.MEDIA_COMMAND_PLAY),
            ).build(),
        )
        assertTrue(media.commands.isEmpty())
        // The host had no session either, so nothing is removed there twice; the word is sent once.
        assertEquals(listOf(MUSIC), sentMediaGone())
    }
}
