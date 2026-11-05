package io.github.thisisthepy.dsa.agent.files

import io.github.thisisthepy.dsa.agent.RecordingLog
import io.github.thisisthepy.dsa.agent.decode
import io.github.thisisthepy.dsa.control.v1.DragEnd
import io.github.thisisthepy.dsa.control.v1.DragOut as DragOutRequest
import io.github.thisisthepy.dsa.control.v1.DragRead
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.PointerTool
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DragOutTest {
    private val log = RecordingLog()
    private val sent = mutableListOf<Envelope>()
    private val content = ByteArray(300_000) { (it % 251).toByte() }
    private val released = mutableListOf<Triple<Int, Float, Float>>()
    private var releasedDrags = 0
    private val openStreams = mutableListOf<InputStream>()
    private var closedStreams = 0
    private var dragging = true

    private val drag = object : GuestDrag {
        override val items = listOf(
            DraggedItem("holiday.jpg", content.size.toLong(), "image/jpeg"),
            DraggedItem("", null, "text/plain", text = "hello"),
        )

        override fun open(index: Int): InputStream {
            val stream = object : ByteArrayInputStream(content) {
                override fun close() {
                    closedStreams++
                    super.close()
                }
            }
            openStreams += stream
            return stream
        }

        override fun release() {
            releasedDrags++
        }
    }

    private val dragOut = DragOut(
        catcher = { _, _, _, release ->
            if (dragging) {
                release()
                drag
            } else {
                null
            }
        },
        releasePointer = { taskId, x, y, _, _ -> released += Triple(taskId, x, y) },
        send = { frame -> sent += decode(frame); true },
        log = log,
        run = { it.run() },
    )

    private fun out(requestId: Int) {
        dragOut.handle(
            Envelope.newBuilder().setDragOut(
                DragOutRequest.newBuilder().setRequestId(requestId).setTaskId(7).setX(300f).setY(2f)
                    .setTool(PointerTool.POINTER_TOOL_MOUSE),
            ).build(),
        )
    }

    private fun read(requestId: Int, dragId: Int, item: Int, offset: Long) {
        dragOut.handle(
            Envelope.newBuilder().setDragRead(
                DragRead.newBuilder().setRequestId(requestId).setDragId(dragId).setItem(item)
                    .setOffset(offset).setLength(256 * 1024),
            ).build(),
        )
    }

    @Test
    fun fr29_a_dragged_file_is_offered_by_name_and_streamed_whole_to_the_host() {
        out(1)
        assertEquals(listOf(Triple(7, 300f, 2f)), released)
        val offer = sent.single().dragOffer
        assertEquals(1, offer.requestId)
        assertEquals("", offer.error)
        assertEquals("holiday.jpg", offer.getItems(0).name)
        assertEquals(content.size.toLong(), offer.getItems(0).size)
        assertTrue(offer.getItems(0).sizeKnown)
        assertTrue(offer.getItems(1).isText)
        assertEquals("hello", offer.getItems(1).text)

        val copied = ByteArrayOutputStream()
        var request = 2
        while (true) {
            read(request, offer.dragId, 0, copied.size().toLong())
            val data = sent.last().dragData
            assertEquals(request, data.requestId)
            assertEquals("", data.error)
            data.data.writeTo(copied)
            request++
            if (data.end) break
        }
        assertArrayEquals(content, copied.toByteArray())
        assertEquals("one stream for a file read in order", 1, openStreams.size)

        dragOut.handle(Envelope.newBuilder().setDragEnd(DragEnd.newBuilder().setDragId(offer.dragId).setDropped(true)).build())
        assertEquals(1, releasedDrags)
        assertEquals(1, closedStreams)
    }

    @Test
    fun fr29_no_drag_is_answered_with_nothing_and_the_pointer_left_alone() {
        dragging = false
        out(1)
        val offer = sent.single().dragOffer
        assertEquals(0, offer.itemsCount)
        assertTrue(released.isEmpty())
    }

    @Test
    fun fr29_a_cancelled_drag_leaves_nothing_open_and_cannot_be_read() {
        out(1)
        val dragId = sent.single().dragOffer.dragId
        read(2, dragId, 0, 0)
        dragOut.handle(Envelope.newBuilder().setDragEnd(DragEnd.newBuilder().setDragId(dragId)).build())
        assertEquals(1, releasedDrags)
        assertEquals(1, closedStreams)
        read(3, dragId, 0, 0)
        assertFalse(sent.last().dragData.error.isEmpty())
        // Text is not a file to read.
        out(4)
        read(5, sent.last().dragOffer.dragId, 1, 0)
        assertFalse(sent.last().dragData.error.isEmpty())
    }
}
