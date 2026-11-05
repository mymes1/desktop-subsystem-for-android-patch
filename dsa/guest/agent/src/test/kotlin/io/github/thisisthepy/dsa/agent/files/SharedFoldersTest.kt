package io.github.thisisthepy.dsa.agent.files

import io.github.thisisthepy.dsa.agent.FakeScheduler
import io.github.thisisthepy.dsa.agent.RecordingLog
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.FileSharing
import java.io.File
import java.nio.file.Files
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SharedFoldersTest {
    private val storage: File = Files.createTempDirectory("dsa-shared-storage").toFile()
    private val scheduler = FakeScheduler()
    private val scanned = mutableListOf<File>()
    private val folders = SharedFolders(storage, { scanned += it }, scheduler, RecordingLog(), retryMillis = 1_000, tries = 3)

    @After
    fun cleanUp() {
        storage.deleteRecursively()
    }

    private fun sharing(on: Boolean, vararg names: String) = Envelope.newBuilder().setFileSharing(
        FileSharing.newBuilder().setOn(on).setPort(5590).setLocation("Windows").addAllFolders(names.toList()),
    ).build()

    private fun mount(name: String) {
        val folder = File(File(storage, "Windows"), name)
        folder.mkdirs()
        File(folder, "photo.jpg").writeBytes(byteArrayOf(1))
    }

    @Test
    fun fr29_a_shared_folder_is_scanned_once_it_is_mounted() {
        assertTrue(folders.handle(sharing(true, "Pictures")))
        assertEquals(emptyList<File>(), scanned)
        mount("Pictures")
        scheduler.advance(1_000)
        assertEquals(listOf(File(File(storage, "Windows"), "Pictures")), scanned)
        scheduler.advance(10_000)
        assertEquals("scanned once", 1, scanned.size)
    }

    @Test
    fun fr29_a_folder_already_there_is_scanned_at_once_and_one_that_never_shows_is_scanned_at_the_end() {
        mount("Documents")
        folders.handle(sharing(true, "Documents", "Music"))
        assertEquals(listOf("Documents"), scanned.map { it.name })
        scheduler.advance(5_000)
        assertEquals(listOf("Documents", "Music"), scanned.map { it.name })
    }

    @Test
    fun fr29_nothing_is_scanned_when_sharing_is_off_or_a_name_leaves_the_storage() {
        folders.handle(sharing(false, "Pictures"))
        folders.handle(sharing(true, "..", "a/b"))
        scheduler.advance(10_000)
        assertEquals(emptyList<File>(), scanned)
        assertFalse(folders.handle(Envelope.getDefaultInstance()))
    }
}
