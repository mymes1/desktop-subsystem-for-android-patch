package io.github.thisisthepy.dsa.agent.platform

import android.app.Activity
import android.content.ContentResolver
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.OpenableColumns
import android.widget.Toast
import io.github.thisisthepy.dsa.agent.R
import io.github.thisisthepy.dsa.agent.files.DraggedItem
import io.github.thisisthepy.dsa.agent.files.GuestDrag
import io.github.thisisthepy.dsa.agent.share.SharedContent
import java.io.InputStream

/**
 * "Share on PC" in Android's share sheet. It takes what an app shares (`ACTION_SEND` and
 * `ACTION_SEND_MULTIPLE`: text, links and files) and hands it to the agent, which sends it to
 * the host's share UI: Windows' share window with Nearby sharing, the macOS share menu, or the
 * desktop portal on Linux. It shows nothing. It stays until the host has copied the shared files,
 * because the app's read access to them lasts only while this activity does, and finishes
 * then; it runs in a task of its own, which the host shows no window for, so the app that
 * shared stays usable meanwhile.
 */
class ShareActivity : Activity() {
    private val main = Handler(Looper.getMainLooper())

    /** The shares this activity holds access for, until the host is done with each. */
    private var open = 0

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        take(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        take(intent)
    }

    private fun take(intent: Intent?) {
        val agent = AgentApplication.agent
        val content = intent?.let {
            val subject = it.getStringExtra(Intent.EXTRA_SUBJECT)
                ?: it.getCharSequenceExtra(Intent.EXTRA_TITLE)?.toString()
            SharedContent.from(it.action, it.type, texts(it), uris(it), subject)
        }
        if (agent == null || content == null || content.isEmpty) {
            if (agent == null) Toast.makeText(this, R.string.share_unavailable, Toast.LENGTH_LONG).show()
            finishIfIdle()
            return
        }
        val (packageName, appLabel) = sharingApp()
        val drag = SharedDrag(contentResolver, content)
        open++
        agent.share(drag, content.title, packageName, appLabel)
        // The host may never end it; the access is given back after a while in any case.
        main.postDelayed({ drag.release() }, HOLD_MS)
    }

    private fun texts(intent: Intent): List<String> {
        val texts = mutableListOf<String>()
        intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.let(texts::add)
        @Suppress("DEPRECATION")
        intent.getCharSequenceArrayListExtra(Intent.EXTRA_TEXT)?.forEach { texts.add(it.toString()) }
        val clip = intent.clipData
        if (clip != null) {
            for (i in 0 until clip.itemCount) {
                val item = clip.getItemAt(i)
                if (item.uri == null) item.text?.toString()?.let { if (it !in texts) texts.add(it) }
            }
        }
        return texts
    }

    private fun uris(intent: Intent): List<Uri> {
        val uris = linkedSetOf<Uri>()
        if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)?.let(uris::addAll)
        } else {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)?.let(uris::add)
        }
        val clip = intent.clipData
        if (clip != null) {
            for (i in 0 until clip.itemCount) clip.getItemAt(i).uri?.let(uris::add)
        }
        return uris.filter { it.scheme == ContentResolver.SCHEME_CONTENT || it.scheme == ContentResolver.SCHEME_FILE }
    }

    /** The app that shared, by package and by the name the guest shows for it. */
    private fun sharingApp(): Pair<String, String> {
        val packageName = launchedFromPackage ?: referrer?.host ?: ""
        if (packageName.isEmpty()) return "" to ""
        val label = try {
            packageManager.getApplicationLabel(
                packageManager.getApplicationInfo(packageName, PackageManager.ApplicationInfoFlags.of(0)),
            ).toString()
        } catch (e: PackageManager.NameNotFoundException) {
            ""
        }
        return packageName to label
    }

    private fun ended() {
        open = (open - 1).coerceAtLeast(0)
        finishIfIdle()
    }

    private fun finishIfIdle() {
        if (open == 0 && !isFinishing) finish()
    }

    /** What an app shared, read from its content providers while this activity holds the access. */
    private inner class SharedDrag(
        private val resolver: ContentResolver,
        private val content: SharedContent<Uri>,
    ) : GuestDrag {
        @Volatile
        private var released = false

        override val items: List<DraggedItem> = content.texts.map { DraggedItem("", null, "text/plain", text = it) } +
            content.files.map { describe(it) }

        private fun describe(uri: Uri): DraggedItem {
            var name = uri.lastPathSegment ?: ""
            var size: Long? = null
            try {
                resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { cursor ->
                    if (cursor.moveToFirst()) {
                        val nameColumn = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                        val sizeColumn = cursor.getColumnIndex(OpenableColumns.SIZE)
                        if (nameColumn >= 0 && !cursor.isNull(nameColumn)) name = cursor.getString(nameColumn)
                        if (sizeColumn >= 0 && !cursor.isNull(sizeColumn)) size = cursor.getLong(sizeColumn)
                    }
                }
            } catch (e: Exception) {
                AndroidLog.warn("could not read the name of a shared file", e)
            }
            val type = try {
                resolver.getType(uri)
            } catch (e: Exception) {
                null
            } ?: content.type ?: "application/octet-stream"
            return DraggedItem(name, size, type)
        }

        override fun open(index: Int): InputStream {
            val uri = content.files[index - content.texts.size]
            return resolver.openInputStream(uri) ?: throw java.io.FileNotFoundException("the app gave no content for the file")
        }

        override fun release() {
            if (released) return
            released = true
            main.post { ended() }
        }
    }

    private companion object {
        /** The longest the activity holds the access to shared files for one share. */
        const val HOLD_MS = 10 * 60 * 1000L
    }
}
