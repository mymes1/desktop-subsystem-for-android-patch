package io.github.thisisthepy.dsa.agent.windowing

import io.github.thisisthepy.dsa.agent.FakeActions
import io.github.thisisthepy.dsa.agent.RecordingLog
import io.github.thisisthepy.dsa.agent.tasks.TaskBounds
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class CaptionInsetsTest {
    private val actions = FakeActions()
    private val log = RecordingLog()
    private val insets = CaptionInsets(actions, log)
    private val reach = CaptionInsets.REACH

    @Test
    fun fr22_a_task_gets_a_top_inset_as_high_as_its_windows_title_bar() {
        insets.setHeight(7, 62, TaskBounds(100, 40, 900, 1040))

        // The band along the task's top, as high as the title bar, reaching past both sides.
        assertEquals(listOf(7 to TaskBounds(-reach, 40, reach, 102)), actions.captionInsets)
    }

    @Test
    fun fr22_the_inset_follows_the_tasks_top_edge_and_the_height() {
        insets.setHeight(7, 31, TaskBounds(100, 40, 900, 1040))
        actions.captionInsets.clear()

        // A change of width alone needs nothing new.
        insets.update(mapOf(7 to TaskBounds(100, 40, 1200, 1040)))
        assertTrue(actions.captionInsets.isEmpty())

        insets.update(mapOf(7 to TaskBounds(100, 90, 1200, 1040)))
        assertEquals(listOf(7 to TaskBounds(-reach, 90, reach, 121)), actions.captionInsets)

        actions.captionInsets.clear()
        insets.setHeight(7, 62, TaskBounds(100, 90, 1200, 1040))
        assertEquals(listOf(7 to TaskBounds(-reach, 90, reach, 152)), actions.captionInsets)

        actions.captionInsets.clear()
        insets.setHeight(7, 0, TaskBounds(100, 90, 1200, 1040))
        assertEquals(listOf<Pair<Int, TaskBounds?>>(7 to null), actions.captionInsets)
    }

    @Test
    fun fr22_an_ended_task_is_forgotten_and_a_task_without_a_height_gets_nothing() {
        insets.update(mapOf(3 to TaskBounds(0, 0, 400, 300)))
        assertTrue(actions.captionInsets.isEmpty())

        insets.setHeight(3, 31, TaskBounds(0, 0, 400, 300))
        insets.update(emptyMap())
        actions.captionInsets.clear()
        insets.update(mapOf(3 to TaskBounds(0, 50, 400, 300)))
        assertTrue("a new task 3 has no height yet", actions.captionInsets.isEmpty())
    }

    @Test
    fun fr22_an_inset_the_framework_refused_is_tried_again_and_logged_once() {
        actions.insetsTaken = false
        insets.setHeight(7, 31, TaskBounds(0, 0, 400, 300))
        insets.update(mapOf(7 to TaskBounds(0, 0, 400, 300)))
        assertEquals(2, actions.captionInsets.size)
        assertEquals(1, log.lines.count { it.startsWith("warn:") })

        actions.insetsTaken = true
        insets.update(mapOf(7 to TaskBounds(0, 0, 400, 300)))
        insets.update(mapOf(7 to TaskBounds(0, 0, 400, 300)))
        assertEquals(3, actions.captionInsets.size)
    }
}
