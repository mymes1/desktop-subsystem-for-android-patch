package io.github.thisisthepy.dsa.agent.files

import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.agent.Scheduler
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.FileSharing
import java.io.File

/** Has the media scanner look at a folder, so the media apps show what is in it. */
fun interface MediaScan {
    fun scan(folder: File)

    companion object {
        val NONE = MediaScan { }
    }
}

/**
 * The host's shared folders, as the media apps see them. The control daemon mounts each
 * folder the host shares under `<storage>/<location>/<folder>` (FileSharing); once a folder
 * shows there, the media scanner is asked to look at it, so the gallery shows the host's
 * pictures without the user opening the folder first. A folder that does not show at once is
 * looked for again a few times, since the mount follows the message by a moment.
 *
 * Every method runs on the agent's scheduler thread.
 */
class SharedFolders(
    private val storage: File,
    private val scanner: MediaScan,
    private val scheduler: Scheduler,
    private val log: Log,
    private val retryMillis: Long = RETRY_MILLIS,
    private val tries: Int = TRIES,
) {
    /** Bumped by each FileSharing, so the looks of an older one stop. */
    private var generation = 0

    /** Takes the host's FileSharing. False for any other frame. */
    fun handle(envelope: Envelope): Boolean {
        if (envelope.bodyCase != Envelope.BodyCase.FILE_SHARING) return false
        shared(envelope.fileSharing)
        return true
    }

    private fun shared(sharing: FileSharing) {
        generation += 1
        if (!sharing.on || !isPlainName(sharing.location)) return
        val mine = generation
        for (folder in sharing.foldersList.filter(::isPlainName)) {
            look(mine, File(File(storage, sharing.location), folder), tries)
        }
    }

    private fun look(mine: Int, folder: File, left: Int) {
        if (mine != generation) return
        val there = folder.isDirectory && (folder.list()?.isNotEmpty() == true)
        if (there || left <= 1) {
            try {
                scanner.scan(folder)
                log.info("asked the media scanner to look at the host's ${folder.name}")
            } catch (e: Exception) {
                log.warn("the media scanner could not be asked to look at ${folder.name}", e)
            }
            return
        }
        scheduler.postDelayed(retryMillis) { look(mine, folder, left - 1) }
    }

    companion object {
        const val RETRY_MILLIS = 2_000L
        const val TRIES = 15

        /** Whether [name] is one plain name, which a shared folder is. */
        fun isPlainName(name: String): Boolean =
            name.isNotEmpty() && name != "." && name != ".." && name.length <= 64 &&
                name.all { it.isLetterOrDigit() || it == ' ' || it == '-' || it == '_' }
    }
}
