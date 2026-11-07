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

    @Test
    fun fr22_an_app_that_draws_its_own_title_bar_is_told_where_the_hosts_buttons_and_icon_are() {
        val at = TaskBounds(100, 40, 900, 1040)
        insets.setHeight(7, 31, at, startWidth = 40, endWidth = 138)
        assertEquals(listOf(emptyList<TaskBounds>()), actions.captionBoundingRects)
        actions.captionInsets.clear()
        actions.captionBoundingRects.clear()

        // It asks for a transparent caption bar: the band is the task's own width, and the
        // caption bar inset carries the icon's cell and the buttons, relative to the band.
        insets.update(mapOf(7 to at), appDrawn = setOf(7))
        assertEquals(listOf(7 to TaskBounds(100, 40, 900, 71)), actions.captionInsets)
        assertEquals(
            listOf(listOf(TaskBounds(0, 0, 40, 31), TaskBounds(662, 0, 800, 31))),
            actions.captionBoundingRects,
        )

        // A change of width moves the buttons' rectangle.
        insets.update(mapOf(7 to TaskBounds(100, 40, 1100, 1040)), appDrawn = setOf(7))
        assertEquals(listOf(TaskBounds(0, 0, 40, 31), TaskBounds(862, 0, 1000, 31)), actions.captionBoundingRects.last())

        // Withdrawn, it gets the plain band again.
        insets.update(mapOf(7 to TaskBounds(100, 40, 1100, 1040)))
        assertEquals(7 to TaskBounds(-reach, 40, reach, 71), actions.captionInsets.last())
        assertEquals(emptyList<TaskBounds>(), actions.captionBoundingRects.last())
    }

    @Test
    fun fr22_an_apps_controls_in_its_title_bar_are_what_it_keeps_gestures_out_of() {
        val at = TaskBounds(100, 40, 900, 1040)
        insets.setHeight(7, 31, at, startWidth = 40, endWidth = 138)
        val exclusion = listOf(
            // A tab strip running under the icon's cell: only the part after it counts.
            TaskBounds(110, 40, 500, 71),
            // A button below the band: none of it counts.
            TaskBounds(600, 200, 700, 260),
            // A control reaching under the caption buttons: cut where they begin.
            TaskBounds(700, 45, 880, 70),
            // Another task's window.
            TaskBounds(1000, 40, 1200, 71),
        )
        assertEquals(
            listOf(TaskBounds(40, 0, 400, 31), TaskBounds(600, 5, 662, 30)),
            insets.controls(7, at, appDrawn = true, exclusion = exclusion),
        )
        // An app that does not draw its own title bar has none there.
        assertTrue(insets.controls(7, at, appDrawn = false, exclusion = exclusion).isEmpty())
        // Nor has a task the host said nothing of.
        assertTrue(insets.controls(8, at, appDrawn = true, exclusion = exclusion).isEmpty())
    }
}
