package io.github.thisisthepy.dsa.agent.platform

import android.graphics.Rect
import android.graphics.Region
import android.graphics.RegionIterator
import android.view.Display
import android.view.ISystemGestureExclusionListener
import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.agent.tasks.TaskBounds
import io.github.thisisthepy.dsa.agent.windowing.GestureExclusion

/**
 * The display's system gesture exclusion, from the window manager
 * (IWindowManager.registerSystemGestureExclusionListener, which needs no permission and is
 * what Android's own desktop windows listen to).
 *
 * The listener hears two regions: the exclusion with the limits the system puts on it near the
 * display's edges for its back gesture, and the one the apps asked for, when those limits cut
 * it. The guest has no back gesture at its edges, so the one the apps asked for is used when
 * there is one. IWindowManager is not in the SDK; it is reached through WindowManagerGlobal by
 * reflection, which a platform app may do.
 */
class AndroidGestureExclusion(private val log: Log) : GestureExclusion {
    private var listener: ISystemGestureExclusionListener.Stub? = null

    override fun listen(onChange: (List<TaskBounds>) -> Unit): Boolean {
        val callback = object : ISystemGestureExclusionListener.Stub() {
            override fun onSystemGestureExclusionChanged(
                displayId: Int,
                systemGestureExclusion: Region?,
                systemGestureExclusionUnrestricted: Region?,
            ) {
                if (displayId != Display.DEFAULT_DISPLAY) return
                val region = systemGestureExclusionUnrestricted ?: systemGestureExclusion
                onChange(rects(region))
            }
        }
        return try {
            val manager = Class.forName("android.view.WindowManagerGlobal")
                .getMethod("getWindowManagerService")
                .invoke(null)
                ?: return false
            Class.forName("android.view.IWindowManager")
                .getMethod("registerSystemGestureExclusionListener", ISystemGestureExclusionListener::class.java, Int::class.javaPrimitiveType)
                .invoke(manager, callback, Display.DEFAULT_DISPLAY)
            listener = callback
            true
        } catch (e: ReflectiveOperationException) {
            log.warn("could not listen to the display's gesture exclusion; apps that draw their own title bar get no controls there", e)
            false
        } catch (e: LinkageError) {
            log.warn("this framework cannot say where apps keep gestures out; apps that draw their own title bar get no controls there", e)
            false
        }
    }

    private fun rects(region: Region?): List<TaskBounds> {
        if (region == null || region.isEmpty) return emptyList()
        val out = mutableListOf<TaskBounds>()
        val iterator = RegionIterator(region)
        val rect = Rect()
        while (iterator.next(rect)) out += TaskBounds(rect.left, rect.top, rect.right, rect.bottom)
        return out
    }
}
