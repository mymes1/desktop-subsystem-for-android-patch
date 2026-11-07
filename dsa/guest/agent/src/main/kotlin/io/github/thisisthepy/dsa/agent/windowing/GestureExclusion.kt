package io.github.thisisthepy.dsa.agent.windowing

import io.github.thisisthepy.dsa.agent.tasks.TaskBounds

/**
 * The parts of the guest's display where apps keep system gestures out
 * (View.setSystemGestureExclusionRects), in display pixels.
 *
 * An app that draws its own title bar marks its controls there this way, and the rest of the
 * band is space that drags the window: this is how Android's own desktop windows tell the two
 * apart in an app's title bar (the window manager shell's WindowDecorationGestureExclusionTracker
 * and CaptionRegionHelper).
 */
fun interface GestureExclusion {
    /**
     * Calls [onChange], on any thread, with the whole display's exclusion each time it changes,
     * the first time soon after it is called. False when the framework cannot say.
     */
    fun listen(onChange: (List<TaskBounds>) -> Unit): Boolean

    companion object {
        /** Says nothing, so no app's title bar has controls the pointer reaches. */
        val NONE = GestureExclusion { false }
    }
}
