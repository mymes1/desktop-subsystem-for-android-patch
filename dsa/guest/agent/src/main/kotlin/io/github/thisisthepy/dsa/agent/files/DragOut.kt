package io.github.thisisthepy.dsa.agent.files

import com.google.protobuf.ByteString
import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.control.v1.DragData
import io.github.thisisthepy.dsa.control.v1.DragItem
import io.github.thisisthepy.dsa.control.v1.DragOffer
import io.github.thisisthepy.dsa.control.v1.DragOut as DragOutRequest
import io.github.thisisthepy.dsa.control.v1.DragRead
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.PointerTool
import java.io.InputStream

/** One thing an app drags: a file (a content URI it lets the drop target read) or text. */
data class DraggedItem(
    /** The file's name as its content provider gives it; empty for text. */
    val name: String,
    /** The file's length in bytes, or null when the provider does not say. */
    val size: Long?,
    val mimeType: String,
    /** The text, for an item that is text rather than a file. */
    val text: String? = null,
)

/** A drag an app started, ended in the guest on the agent's window and held open for the host. */
interface GuestDrag {
    val items: List<DraggedItem>

    /** The content of file item [index], from its start. */
    fun open(index: Int): InputStream

    /** Gives back the access the drop granted, so nothing of the drag stays open. */
    fun release()
}

/** The agent's window that catches a global drag as the pointer leaves an app's window. */
fun interface DragCatcher {
    /**
     * If an app is dragging with DRAG_FLAG_GLOBAL, puts the agent's window under the pointer,
     * calls [release] to lift the pointer there at ([x], [y]) of task [taskId], and returns the
     * drag once it is dropped on that window; null when nothing is being dragged or the drop
     * did not come.
     */
    fun catchDrag(taskId: Int, x: Float, y: Float, release: () -> Unit): GuestDrag?

    companion object {
        val NONE = DragCatcher { _, _, _, _ -> null }
    }
}

/**
 * Hands a drag out of an app's window to the host, as a native drag there.
 *
 * The host sends DragOut when the pointer leaves a task's window with a button held. The agent
 * asks its [DragCatcher] to end the app's drag on the agent's own window, which takes read
 * access to what it carries, and answers DragOffer with each item's name, size and type, or
 * its text. Content URIs and paths stay in the guest. Each DragRead is then answered with the
 * next part of a file, read from the app's content provider as the host app it was dropped on
 * copies it. DragEnd closes everything and gives the access back. One drag is open at a time:
 * a new one ends the last.
 *
 * The work runs on [run], a thread of its own, because catching a drop waits for the window
 * manager and reading waits for the app's provider, and neither may hold up the agent.
 */
class DragOut(
    private val catcher: DragCatcher,
    /** Lifts the pointer, as the host's own pointer UP would, so the guest's drag ends. */
    private val releasePointer: (taskId: Int, x: Float, y: Float, tool: PointerTool, pointerId: Int) -> Unit,
    private val send: (ByteArray) -> Boolean,
    private val log: Log,
    private val run: (Runnable) -> Unit,
) {
    private class Open(val id: Int, val drag: GuestDrag) {
        var reading: Int = -1
        var stream: InputStream? = null
        var position: Long = 0
    }

    private var open: Open? = null
    private var lastId = 0

    /** Takes a DragOut, DragRead or DragEnd frame. False for any other frame. */
    fun handle(envelope: Envelope): Boolean {
        when (envelope.bodyCase) {
            Envelope.BodyCase.DRAG_OUT -> {
                val request = envelope.dragOut
                run(Runnable { offer(request) })
            }
            Envelope.BodyCase.DRAG_READ -> {
                val read = envelope.dragRead
                run(Runnable { read(read) })
            }
            Envelope.BodyCase.DRAG_END -> {
                val dragId = envelope.dragEnd.dragId
                run(Runnable { end(dragId) })
            }
            else -> return false
        }
        return true
    }

    private fun offer(request: DragOutRequest) {
        open?.let { close(it) }
        open = null
        val answer = DragOffer.newBuilder().setRequestId(request.requestId)
        val drag = try {
            catcher.catchDrag(request.taskId, request.x, request.y) {
                releasePointer(request.taskId, request.x, request.y, request.tool, request.pointerId)
            }
        } catch (e: Exception) {
            log.warn("could not take the drag out of task ${request.taskId}", e)
            answer.setError("Android could not hand the drag over: ${e.message ?: e.javaClass.simpleName}")
            null
        }
        if (drag != null) {
            val items = drag.items
            if (items.isEmpty()) {
                drag.release()
            } else {
                lastId = if (lastId == Int.MAX_VALUE) 1 else lastId + 1
                open = Open(lastId, drag)
                answer.setDragId(lastId)
                items.forEach { answer.addItems(toWire(it)) }
                log.info("drag out of task ${request.taskId}: ${items.size} item(s) for the host")
            }
        }
        send(Envelope.newBuilder().setDragOffer(answer).build().toByteArray())
    }

    private fun toWire(item: DraggedItem): DragItem {
        val wire = DragItem.newBuilder().setMimeType(item.mimeType)
        if (item.text != null) return wire.setIsText(true).setText(item.text).build()
        wire.setName(item.name)
        if (item.size != null && item.size >= 0) wire.setSize(item.size).setSizeKnown(true)
        return wire.build()
    }

    private fun read(read: DragRead) {
        val answer = DragData.newBuilder().setRequestId(read.requestId)
        val drag = open
        try {
            require(drag != null && drag.id == read.dragId) { "the drag is over" }
            val index = read.item
            val item = drag.drag.items.getOrNull(index)
            require(item != null && item.text == null) { "the drag has no file $index" }
            if (drag.reading != index || drag.position != read.offset) {
                // A host app that reads again from the start, or another file, gets a new stream.
                drag.stream?.close()
                drag.stream = drag.drag.open(index)
                drag.reading = index
                drag.position = 0
                skip(drag.stream!!, read.offset)
                drag.position = read.offset
            }
            val stream = drag.stream!!
            val wanted = read.length.coerceIn(1, MAX_READ)
            val buffer = ByteArray(wanted)
            var filled = 0
            var ended = false
            while (filled < wanted) {
                val n = stream.read(buffer, filled, wanted - filled)
                if (n < 0) {
                    ended = true
                    break
                }
                filled += n
            }
            drag.position += filled
            if (item.size != null && drag.position >= item.size) ended = true
            answer.setData(ByteString.copyFrom(buffer, 0, filled)).setEnd(ended)
        } catch (e: Exception) {
            log.warn("could not read a dragged file for the host", e)
            answer.setError("the dragged file could not be read: ${e.message ?: e.javaClass.simpleName}")
        }
        send(Envelope.newBuilder().setDragData(answer).build().toByteArray())
    }

    private fun skip(stream: InputStream, count: Long) {
        var left = count
        val scratch = ByteArray(64 * 1024)
        while (left > 0) {
            val n = stream.read(scratch, 0, minOf(left, scratch.size.toLong()).toInt())
            require(n >= 0) { "the dragged file is shorter than the host asked for" }
            left -= n
        }
    }

    private fun end(dragId: Int) {
        val drag = open ?: return
        if (drag.id != dragId) return
        close(drag)
        open = null
    }

    private fun close(drag: Open) {
        try {
            drag.stream?.close()
        } catch (e: Exception) {
            log.warn("could not close a dragged file", e)
        }
        drag.stream = null
        try {
            drag.drag.release()
        } catch (e: Exception) {
            log.warn("could not give back the access to a drag", e)
        }
    }

    private companion object {
        /** The most one answer carries: 256 KiB, as the host asks for at most. */
        const val MAX_READ = 256 * 1024
    }
}
