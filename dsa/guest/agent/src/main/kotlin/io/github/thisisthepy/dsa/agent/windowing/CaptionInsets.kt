package io.github.thisisthepy.dsa.agent.windowing

import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.agent.requests.TaskActions
import io.github.thisisthepy.dsa.agent.tasks.TaskBounds

/**
 * Gives each task a top inset as high as the title bar its host window draws over it.
 *
 * The host's window has no title bar above the app: the app's frames fill the window from
 * its top edge, and the window draws its caption buttons over them. The host says how high
 * that band is ([setHeight]); the task gets an inset source of that height along its top,
 * which its app receives as its status bar, system bar and caption bar inset. The app then
 * keeps its toolbar below the buttons and draws its status bar background behind them.
 *
 * The framework takes the inset's frame in display pixels, so it is given again whenever the
 * task's top edge moves ([update]). The frame reaches far past the task on both sides: only
 * its overlap with the task's windows counts, so a task that only changes width needs nothing
 * new.
 *
 * Not thread safe: the agent calls it from one thread.
 */
class CaptionInsets(
    private val actions: TaskActions,
    private val log: Log,
) {
    private val heights = HashMap<Int, Int>()

    /** The frame each task was last given; null once its inset was removed. */
    private val applied = HashMap<Int, TaskBounds?>()
    private var warned = false

    /** The host's window draws its title bar over the task's top [height] pixels. */
    fun setHeight(taskId: Int, height: Int, bounds: TaskBounds?) {
        heights[taskId] = height
        apply(taskId, bounds)
    }

    /** The tasks the host was told about, where they are now. Forgets the tasks that ended. */
    fun update(bounds: Map<Int, TaskBounds>) {
        heights.keys.retainAll(bounds.keys)
        applied.keys.retainAll(bounds.keys)
        for ((taskId, at) in bounds) {
            if (taskId in heights) apply(taskId, at)
        }
    }

    private fun apply(taskId: Int, bounds: TaskBounds?) {
        val height = heights[taskId] ?: return
        if (bounds == null || bounds.isEmpty()) return
        val frame = if (height > 0) frameOf(bounds, height) else null
        if (taskId in applied && applied[taskId] == frame) return
        if (frame == null && taskId !in applied) {
            applied[taskId] = null
            return
        }
        if (actions.setCaptionInset(taskId, frame)) {
            applied[taskId] = frame
        } else if (!warned) {
            warned = true
            log.warn("could not give task $taskId a top inset for its window's title bar; its app draws under the caption buttons")
        }
    }

    companion object {
        /** How far the frame reaches to each side: past any display. */
        const val REACH = 1 shl 20

        /** The inset's frame for a task at [bounds] under a title bar [height] pixels high. */
        fun frameOf(bounds: TaskBounds, height: Int) = TaskBounds(-REACH, bounds.top, REACH, bounds.top + height)
    }
}
