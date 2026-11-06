package io.github.thisisthepy.dsa.agent.tasks

import io.github.thisisthepy.dsa.agent.decode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TaskWindowModeTest {
    private fun task(
        orientation: TaskOrientation = TaskOrientation.UNSPECIFIED,
        fullscreen: Boolean = false,
        pictureInPicture: Boolean = false,
    ) = TaskSnapshot(
        taskId = 4,
        kind = TaskKind.APP,
        packageName = "org.example.reader",
        label = "Reader",
        bounds = TaskBounds(0, 0, 600, 1000),
        focused = true,
        orientation = orientation,
        fullscreen = fullscreen,
        pictureInPicture = pictureInPicture,
    )

    @Test
    fun fr30_an_activitys_screen_orientation_says_which_way_its_task_is_locked() {
        assertEquals(TaskOrientation.PORTRAIT, TaskOrientation.of(1))
        assertEquals(TaskOrientation.PORTRAIT, TaskOrientation.of(7))
        assertEquals(TaskOrientation.LANDSCAPE, TaskOrientation.of(0))
        assertEquals(TaskOrientation.LANDSCAPE, TaskOrientation.of(6))
        // Unspecified, sensor, user, full sensor and locked do not lock it one way.
        for (free in listOf(-1, 2, 4, 10, 14)) {
            assertEquals(TaskOrientation.UNSPECIFIED, TaskOrientation.of(free))
        }
    }

    @Test
    fun fr30_the_host_hears_a_tasks_orientation_and_whether_it_fills_the_screen_or_floats() {
        val portrait = decode(Messages.taskUpdated(task(TaskOrientation.PORTRAIT))).taskUpdated
        assertEquals(
            io.github.thisisthepy.dsa.control.v1.TaskOrientation.TASK_ORIENTATION_PORTRAIT,
            portrait.orientation,
        )
        assertFalse(portrait.fullscreen)
        val full = decode(Messages.taskUpdated(task(fullscreen = true))).taskUpdated
        assertTrue(full.fullscreen)
        val pip = decode(Messages.taskUpdated(task(pictureInPicture = true))).taskUpdated
        assertTrue(pip.pictureInPicture)
    }

    @Test
    fun fr30_a_change_to_a_tasks_mode_is_news_to_the_host() {
        val sent = mutableListOf<ByteArray>()
        val reporter = TaskReporter("io.github.thisisthepy.dsa.agent") { frame ->
            sent += frame
            true
        }
        reporter.update(listOf(task()))
        reporter.update(listOf(task(fullscreen = true)))
        assertEquals(2, sent.count { decode(it).hasTaskUpdated() })
        assertTrue(decode(sent.last()).taskUpdated.fullscreen)
    }
}
