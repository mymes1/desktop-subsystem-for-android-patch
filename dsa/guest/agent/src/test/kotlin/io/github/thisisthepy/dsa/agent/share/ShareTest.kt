package io.github.thisisthepy.dsa.agent.share

import io.github.thisisthepy.dsa.agent.RecordingLog
import io.github.thisisthepy.dsa.agent.decode
import io.github.thisisthepy.dsa.agent.files.DragOut
import io.github.thisisthepy.dsa.agent.files.DraggedItem
import io.github.thisisthepy.dsa.agent.files.GuestDrag
import io.github.thisisthepy.dsa.control.v1.DragEnd
import io.github.thisisthepy.dsa.control.v1.DragRead
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.ShareOutcome
import io.github.thisisthepy.dsa.control.v1.ShareResult
import java.io.ByteArrayInputStream
import java.io.InputStream
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** "Share on PC": what an app shares goes to the host's share UI, its files read like a drag's. */
class ShareTest {
    private val log = RecordingLog()
    private val sent = mutableListOf<Envelope>()
    private val notices = mutableListOf<String>()
    private var connected = true
    private val photo = "JPEG photo bytes".toByteArray()

    private val dragOut = DragOut(
        catcher = { _, _, _, _ -> null },
        releasePointer = { _, _, _, _, _ -> },
        send = { frame -> sent += decode(frame); true },
        log = log,
        run = { it.run() },
        notify = { notices += it },
    )

    private inner class Shared(content: SharedContent<String>) : GuestDrag {
        private val files = content.files
        private val textCount = content.texts.size
        var released = 0
        override val items: List<DraggedItem> =
            content.texts.map { DraggedItem("", null, "text/plain", text = it) } +
                content.files.map { DraggedItem(it, photo.size.toLong(), "image/jpeg") }

        override fun open(index: Int): InputStream {
            check(files[index - textCount] == "photo.jpg")
            return ByteArrayInputStream(photo)
        }

        override fun release() {
            released++
        }
    }

    private fun shareIntent(): SharedContent<String> = SharedContent.from(
        SharedContent.ACTION_SEND_MULTIPLE,
        "*/*",
        listOf("Look at this", "https://example.com/trip", "  ", "Look at this"),
        listOf("photo.jpg", "photo.jpg"),
        " Our trip ",
    )!!

    @Test
    fun fr36_sharing_hands_content_to_the_host_share_ui() {
        val content = shareIntent()
        assertEquals("Our trip", content.title)
        assertEquals(listOf("Look at this", "https://example.com/trip"), content.texts)
        assertEquals(listOf("photo.jpg"), content.files)
        assertNull("a wildcard names no file's type", content.type)

        val shared = Shared(content)
        dragOut.share(shared, content.title, "com.example.gallery", "Gallery") { connected }
        val share = sent.single().shareToHost
        assertEquals("Our trip", share.title)
        assertEquals("com.example.gallery", share.packageName)
        assertEquals("Gallery", share.appLabel)
        assertEquals(3, share.itemsCount)
        assertTrue(share.getItems(0).isText)
        assertEquals("Look at this", share.getItems(0).text)
        assertEquals("https://example.com/trip", share.getItems(1).text)
        assertEquals("photo.jpg", share.getItems(2).name)
        assertEquals(photo.size.toLong(), share.getItems(2).size)

        // The host reads the file under the share's ID, as it reads a drag's.
        dragOut.handle(
            Envelope.newBuilder().setDragRead(
                DragRead.newBuilder().setRequestId(4).setDragId(share.shareId).setItem(2)
                    .setOffset(0).setLength(256 * 1024),
            ).build(),
        )
        val data = sent.last().dragData
        assertEquals(4, data.requestId)
        assertEquals("", data.error)
        assertTrue(data.end)
        assertArrayEquals(photo, data.data.toByteArray())

        // Once the host has the files, the app's access is given back.
        dragOut.handle(
            Envelope.newBuilder().setDragEnd(DragEnd.newBuilder().setDragId(share.shareId).setDropped(true)).build(),
        )
        assertEquals(1, shared.released)
        dragOut.handle(
            Envelope.newBuilder().setShareResult(
                ShareResult.newBuilder().setShareId(share.shareId).setOutcome(ShareOutcome.SHARE_OUTCOME_SHARED),
            ).build(),
        )
        assertTrue(notices.isEmpty())
    }

    @Test
    fun fr36_a_share_the_pc_refuses_is_told_to_the_user_and_its_access_given_back() {
        val content = shareIntent()
        val shared = Shared(content)
        dragOut.share(shared, content.title, "", "") { connected }
        val shareId = sent.single().shareToHost.shareId
        dragOut.handle(
            Envelope.newBuilder().setShareResult(
                ShareResult.newBuilder().setShareId(shareId).setOutcome(ShareOutcome.SHARE_OUTCOME_FAILED)
                    .setMessage("no desktop session is open"),
            ).build(),
        )
        assertEquals(listOf("no desktop session is open"), notices)
        assertEquals(1, shared.released)
    }

    @Test
    fun fr36_a_share_while_the_pc_cannot_be_reached_is_not_sent() {
        connected = false
        val content = shareIntent()
        val shared = Shared(content)
        dragOut.share(shared, content.title, "", "") { connected }
        assertTrue(sent.isEmpty())
        assertEquals(listOf(""), notices)
        assertEquals(1, shared.released)
    }

    @Test
    fun fr36_only_a_share_intent_is_shared() {
        assertNull(SharedContent.from("android.intent.action.VIEW", null, listOf("x"), emptyList<String>(), null))
        val empty = SharedContent.from(SharedContent.ACTION_SEND, "text/plain", listOf(" "), emptyList<String>(), null)!!
        assertTrue(empty.isEmpty)
    }

    @Test
    fun fr36_shares_stay_open_beside_each_other_up_to_a_limit() {
        val drags = (0..DragOut.MAX_SHARES).map { Shared(shareIntent()) }
        drags.forEach { dragOut.share(it, "", "", "") { connected } }
        assertEquals("the oldest share was ended", 1, drags.first().released)
        assertTrue(drags.drop(1).all { it.released == 0 })
    }
}
