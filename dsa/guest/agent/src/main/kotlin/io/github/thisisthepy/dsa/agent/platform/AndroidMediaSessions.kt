package io.github.thisisthepy.dsa.agent.platform

import android.app.ActivityManager
import android.content.ComponentName
import android.content.Context
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.media.MediaMetadata
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.os.Handler
import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.agent.background.AppStopper
import io.github.thisisthepy.dsa.agent.background.BackgroundApps
import io.github.thisisthepy.dsa.agent.background.MediaListener
import io.github.thisisthepy.dsa.agent.background.MediaSource
import io.github.thisisthepy.dsa.agent.background.MediaState
import io.github.thisisthepy.dsa.agent.background.Playback
import io.github.thisisthepy.dsa.control.v1.MediaCommand
import java.io.ByteArrayOutputStream

/**
 * The guest's active media sessions, through MediaSessionManager.
 *
 * Android lets a notification listener read every app's active session, which is how the
 * agent reaches them: it names its own listener ([HostNotificationListener]), which it has
 * allowed at start. Each session's controller is watched for its metadata and playback, and
 * a media key from the host goes to the session's transport controls, as a headset's would.
 *
 * Every call comes on [handler]'s thread, the agent's.
 */
class AndroidMediaSessions(
    private val context: Context,
    private val log: Log,
    private val handler: Handler,
) : MediaSource {
    private val manager = context.getSystemService(MediaSessionManager::class.java)
    private val listenerComponent = ComponentName(context, HostNotificationListener::class.java)
    private val controllers = mutableMapOf<String, Watched>()
    private var listener: MediaListener? = null

    private inner class Watched(val controller: MediaController) : MediaController.Callback() {
        override fun onMetadataChanged(metadata: MediaMetadata?) = report(controller)
        override fun onPlaybackStateChanged(state: PlaybackState?) = report(controller)
        override fun onSessionDestroyed() {
            handler.post { sessionsChanged(activeControllers()) }
        }
    }

    override fun listen(listener: MediaListener) {
        this.listener = listener
        handler.post {
            try {
                manager.addOnActiveSessionsChangedListener(
                    { controllers -> sessionsChanged(controllers.orEmpty()) },
                    listenerComponent,
                    handler,
                )
            } catch (e: SecurityException) {
                log.warn("Android refused the agent its media sessions; media keys do not reach its apps", e)
                return@post
            }
            sessionsChanged(activeControllers())
        }
    }

    private fun activeControllers(): List<MediaController> = try {
        manager.getActiveSessions(listenerComponent)
    } catch (e: SecurityException) {
        log.warn("Android refused the agent its media sessions", e)
        emptyList()
    }

    /** Follows the sessions Android says are active now: one per package, the first it lists. */
    private fun sessionsChanged(active: List<MediaController>) {
        val byPackage = LinkedHashMap<String, MediaController>()
        for (controller in active) byPackage.putIfAbsent(controller.packageName, controller)
        for ((packageName, watched) in controllers.toMap()) {
            val now = byPackage[packageName]
            if (now == null || now.sessionToken != watched.controller.sessionToken) {
                watched.controller.unregisterCallback(watched)
                controllers.remove(packageName)
                if (now == null) listener?.gone(packageName)
            }
        }
        for ((packageName, controller) in byPackage) {
            if (packageName in controllers) continue
            val watched = Watched(controller)
            controller.registerCallback(watched, handler)
            controllers[packageName] = watched
            report(controller)
        }
    }

    private fun report(controller: MediaController) {
        val state = try {
            state(controller)
        } catch (e: Exception) {
            log.warn("could not read the media session of ${controller.packageName}", e)
            return
        }
        listener?.changed(state)
    }

    override fun active(): List<MediaState> = controllers.values.mapNotNull {
        try {
            state(it.controller)
        } catch (e: Exception) {
            null
        }
    }

    override fun control(packageName: String, command: MediaCommand): Boolean {
        val controls = controllers[packageName]?.controller?.transportControls ?: return false
        when (command) {
            MediaCommand.MEDIA_COMMAND_PLAY -> controls.play()
            MediaCommand.MEDIA_COMMAND_PAUSE -> controls.pause()
            MediaCommand.MEDIA_COMMAND_NEXT -> controls.skipToNext()
            MediaCommand.MEDIA_COMMAND_PREVIOUS -> controls.skipToPrevious()
            MediaCommand.MEDIA_COMMAND_STOP -> controls.stop()
            else -> return true
        }
        return true
    }

    private fun state(controller: MediaController): MediaState {
        val metadata = controller.metadata
        val playback = controller.playbackState
        val actions = playback?.actions ?: 0L
        val playing = playback?.state == PlaybackState.STATE_PLAYING ||
            playback?.state == PlaybackState.STATE_BUFFERING
        val text = { key: String -> metadata?.getText(key)?.toString().orEmpty() }
        return MediaState(
            packageName = controller.packageName,
            appLabel = label(controller.packageName),
            title = text(MediaMetadata.METADATA_KEY_TITLE).ifEmpty { text(MediaMetadata.METADATA_KEY_DISPLAY_TITLE) },
            artist = text(MediaMetadata.METADATA_KEY_ARTIST).ifEmpty { text(MediaMetadata.METADATA_KEY_ALBUM_ARTIST) },
            album = text(MediaMetadata.METADATA_KEY_ALBUM),
            artPng = art(metadata),
            playback = when {
                playing -> Playback.PLAYING
                playback?.state == PlaybackState.STATE_PAUSED -> Playback.PAUSED
                else -> Playback.STOPPED
            },
            canPlay = actions and (PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PLAY_PAUSE) != 0L,
            canPause = actions and (PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_PLAY_PAUSE) != 0L,
            canNext = actions and PlaybackState.ACTION_SKIP_TO_NEXT != 0L,
            canPrevious = actions and PlaybackState.ACTION_SKIP_TO_PREVIOUS != 0L,
        )
    }

    private fun label(packageName: String): String = try {
        val pm = context.packageManager
        pm.getApplicationLabel(pm.getApplicationInfo(packageName, 0)).toString()
    } catch (e: PackageManager.NameNotFoundException) {
        packageName
    }

    /** The album art, scaled to fit [BackgroundApps.ART_PIXELS], as a PNG; empty for none. */
    private fun art(metadata: MediaMetadata?): ByteArray {
        val bitmap = metadata?.getBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART)
            ?: metadata?.getBitmap(MediaMetadata.METADATA_KEY_ART)
            ?: metadata?.getBitmap(MediaMetadata.METADATA_KEY_DISPLAY_ICON)
            ?: return ByteArray(0)
        val side = BackgroundApps.ART_PIXELS
        val scale = minOf(1f, side.toFloat() / maxOf(bitmap.width, bitmap.height, 1))
        val scaled = if (scale < 1f) {
            Bitmap.createScaledBitmap(
                bitmap,
                maxOf(1, (bitmap.width * scale).toInt()),
                maxOf(1, (bitmap.height * scale).toInt()),
                true,
            )
        } else {
            bitmap
        }
        val out = ByteArrayOutputStream()
        scaled.compress(Bitmap.CompressFormat.PNG, 100, out)
        if (scaled !== bitmap) scaled.recycle()
        return out.toByteArray()
    }
}

/**
 * Stops an app as Settings' "Force stop" does, through ActivityManager.forceStopPackage: its
 * services end, its processes die and its alarms are cancelled. The call is not in the SDK
 * and is made by reflection; the agent is platform-signed and holds FORCE_STOP_PACKAGES.
 */
class AndroidAppStopper(private val context: Context, private val log: Log) : AppStopper {
    override fun stop(packageName: String): Boolean {
        val manager = context.getSystemService(ActivityManager::class.java)
        return try {
            ActivityManager::class.java
                .getMethod("forceStopPackage", String::class.java)
                .invoke(manager, packageName)
            true
        } catch (e: ReflectiveOperationException) {
            log.warn("could not stop $packageName", e)
            false
        } catch (e: SecurityException) {
            log.warn("Android refused to stop $packageName", e)
            false
        }
    }
}
