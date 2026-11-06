package io.github.thisisthepy.dsa.agent.platform

import android.content.ClipData
import android.content.Context
import android.graphics.PixelFormat
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.OpenableColumns
import android.view.DragEvent
import android.view.Gravity
import android.view.View
import android.view.ViewTreeObserver
import android.view.WindowManager
import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.agent.files.DragCatcher
import io.github.thisisthepy.dsa.agent.files.DraggedItem
import io.github.thisisthepy.dsa.agent.files.GuestDrag
import java.io.InputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/**
 * The agent's window that catches a global drag as it leaves an app's window for the host.
 *
 * It is a one-pixel window above every app, which takes no input. With the window manager's
 * private flag for intercepting global drags (the one the system's own drag and drop shell
 * uses, which needs MANAGE_ACTIVITY_TASKS), it hears of every drag an app starts with
 * DRAG_FLAG_GLOBAL, even while it covers nothing. When the host says the pointer left a
 * window, the window grows to cover the display and takes input, the pointer is lifted, and
 * the drag drops on it. The drop's permissions are taken for the agent's process
 * (DragAndDropPermissions.takeTransient, which the framework calls for a window with no
 * activity), each item is read through the agent's ContentResolver, and the window shrinks
 * back. Releasing the drag gives the permissions back.
 */
class AndroidDragCatcher(private val context: Context, private val log: Log) : DragCatcher {
    private val main = Handler(Looper.getMainLooper())
    private val wm = context.getSystemService(WindowManager::class.java)

    @Volatile
    private var dragging = false
    private val dropped = AtomicReference<CountDownLatch?>(null)
    private val drop = AtomicReference<GuestDrag?>(null)
    private var view: View? = null
    private var params: WindowManager.LayoutParams? = null

    /** Adds the window. Called once, from any thread. */
    fun start() {
        main.post {
            try {
                add()
            } catch (e: Exception) {
                log.warn("could not add the window that hands drags to the host; apps cannot drag files out", e)
            }
        }
    }

    private fun add() {
        val catcher = View(context)
        catcher.setOnDragListener { _, event -> onDrag(event) }
        val params = WindowManager.LayoutParams(
            1,
            1,
            TYPE_SECURE_SYSTEM_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            title = "DsaDragCatcher"
            setFitInsetsTypes(0)
            setFitInsetsSides(0)
        }
        interceptGlobalDrags(params)
        wm.addView(catcher, params)
        this.view = catcher
        this.params = params
    }

    /** Sets PRIVATE_FLAG_INTERCEPT_GLOBAL_DRAG_AND_DROP, which is not in the SDK. */
    private fun interceptGlobalDrags(params: WindowManager.LayoutParams) {
        try {
            val flag = WindowManager.LayoutParams::class.java
                .getField("PRIVATE_FLAG_INTERCEPT_GLOBAL_DRAG_AND_DROP").getInt(null)
            val field = WindowManager.LayoutParams::class.java.getField("privateFlags")
            field.setInt(params, field.getInt(params) or flag)
        } catch (e: ReflectiveOperationException) {
            log.warn("the window manager has no flag to hear of every drag; only drags over the agent's window reach the host", e)
        }
    }

    private fun onDrag(event: DragEvent): Boolean {
        when (event.action) {
            DragEvent.ACTION_DRAG_STARTED -> dragging = true
            DragEvent.ACTION_DROP -> {
                drop.set(take(event))
                dropped.get()?.countDown()
            }
            DragEvent.ACTION_DRAG_ENDED -> {
                dragging = false
                dropped.get()?.countDown()
            }
        }
        // True for every event, so the window is a target for every drag it hears of.
        return true
    }

    override fun catchDrag(taskId: Int, x: Float, y: Float, release: () -> Unit): GuestDrag? {
        if (!dragging) return null
        val latch = CountDownLatch(1)
        drop.set(null)
        dropped.set(latch)
        try {
            if (!onMain { cover(true) }) return null
            release()
            latch.await(DROP_WAIT_MS, TimeUnit.MILLISECONDS)
            return drop.getAndSet(null)
        } finally {
            dropped.set(null)
            onMain { cover(false) }
        }
    }

    /** Grows the window over the display and lets it take input, or shrinks it back. */
    private fun cover(whole: Boolean): Boolean {
        val view = view ?: return false
        val params = params ?: return false
        params.width = if (whole) WindowManager.LayoutParams.MATCH_PARENT else 1
        params.height = if (whole) WindowManager.LayoutParams.MATCH_PARENT else 1
        params.flags = if (whole) {
            params.flags and WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE.inv()
        } else {
            params.flags or WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE
        }
        wm.updateViewLayout(view, params)
        return true
    }

    /**
     * Runs [work] on the main thread and waits for it and for the window's next layout, so the
     * window manager has the new size before the pointer lifts.
     */
    private fun onMain(work: () -> Boolean): Boolean {
        val done = CountDownLatch(1)
        val result = AtomicReference(false)
        main.post {
            try {
                result.set(work())
            } catch (e: Exception) {
                log.warn("could not move the window that hands drags to the host", e)
            }
            val view = view
            if (view == null || !result.get()) {
                done.countDown()
            } else {
                view.viewTreeObserver.addOnGlobalLayoutListener(object : ViewTreeObserver.OnGlobalLayoutListener {
                    override fun onGlobalLayout() {
                        view.viewTreeObserver.removeOnGlobalLayoutListener(this)
                        done.countDown()
                    }
                })
                view.requestLayout()
            }
        }
        done.await(LAYOUT_WAIT_MS, TimeUnit.MILLISECONDS)
        // The layout reaches the window manager's input windows a frame later.
        Thread.sleep(INPUT_SETTLE_MS)
        return result.get()
    }

    /** Takes the drop's permissions and reads what it carries. Runs on the main thread. */
    private fun take(event: DragEvent): GuestDrag? {
        val clip: ClipData = event.clipData ?: return null
        val permissions = takePermissions(event)
        val resolver = context.contentResolver
        val description = event.clipDescription
        val items = mutableListOf<DraggedItem>()
        val uris = mutableListOf<Uri?>()
        for (index in 0 until clip.itemCount) {
            val item = clip.getItemAt(index)
            val uri = item.uri
            if (uri != null) {
                var name = ""
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
                    log.warn("could not ask an app's provider what a dragged file is", e)
                }
                if (name.isEmpty()) name = uri.lastPathSegment.orEmpty()
                val type = resolver.getType(uri) ?: description?.let { if (it.mimeTypeCount > 0) it.getMimeType(0) else null }
                items += DraggedItem(name, size, type ?: "application/octet-stream")
                uris += uri
            } else {
                val text = item.text ?: item.htmlText ?: continue
                items += DraggedItem("", null, "text/plain", text = text.toString())
                uris += null
            }
        }
        return object : GuestDrag {
            override val items: List<DraggedItem> = items

            override fun open(index: Int): InputStream {
                val uri = uris[index] ?: throw IllegalArgumentException("item $index is text")
                return resolver.openInputStream(uri) ?: throw IllegalStateException("the app's provider gave no content")
            }

            override fun release() {
                try {
                    permissions?.javaClass?.getMethod("release")?.invoke(permissions)
                } catch (e: ReflectiveOperationException) {
                    log.warn("could not give back the access to a drag", e)
                }
            }
        }
    }

    /**
     * DragAndDropPermissions.obtain(event).takeTransient(): read access to the drag's URIs for
     * this process until release. Neither is in the SDK; a platform app may call them.
     */
    private fun takePermissions(event: DragEvent): Any? = try {
        val type = Class.forName("android.view.DragAndDropPermissions")
        val permissions = type.getMethod("obtain", DragEvent::class.java).invoke(null, event)
        if (permissions != null) type.getMethod("takeTransient").invoke(permissions)
        permissions
    } catch (e: ReflectiveOperationException) {
        log.warn("could not take the access an app granted to its drag; its files may not be readable", e)
        null
    }

    private companion object {
        /** WindowManager.LayoutParams.TYPE_SECURE_SYSTEM_OVERLAY, which is not in the SDK. */
        const val TYPE_SECURE_SYSTEM_OVERLAY = 2015
        const val DROP_WAIT_MS = 2_000L
        const val LAYOUT_WAIT_MS = 500L
        const val INPUT_SETTLE_MS = 50L
    }
}
