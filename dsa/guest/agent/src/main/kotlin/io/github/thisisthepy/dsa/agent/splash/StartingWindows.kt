package io.github.thisisthepy.dsa.agent.splash

/** A starting window on screen. */
interface StartingWindow {
    fun remove()
}

/**
 * The starting window of each task. A task has at most one: a new one replaces the old. The
 * window manager asks for a removal by task, also for a task the agent drew nothing for.
 */
class StartingWindows {
    private val windows = mutableMapOf<Int, StartingWindow>()

    fun tasks(): Set<Int> = windows.keys.toSet()

    /**
     * Shows the window [create] makes for [taskId], after removing the task's old one. A
     * null window (it could not be added) is not kept. Returns what failed to be removed.
     */
    fun add(taskId: Int, create: () -> StartingWindow?): List<Throwable> {
        val errors = remove(taskId)
        create()?.let { windows[taskId] = it }
        return errors
    }

    /** Removes the task's window, if it has one. Returns the error removing it threw, if any. */
    fun remove(taskId: Int): List<Throwable> {
        val window = windows.remove(taskId) ?: return emptyList()
        return listOfNotNull(runCatching { window.remove() }.exceptionOrNull())
    }

    /** Removes every window; one that fails does not keep the others. */
    fun clear(): List<Throwable> = windows.keys.toList().flatMap { remove(it) }
}

/** The surface calls [TaskLeashes] makes. */
interface LeashSurfaces<L> {
    /** Puts [leash] at ([x], [y]) in its parent and crops it to [width] by [height] from there. */
    fun place(leash: L, x: Int, y: Int, width: Int, height: Int)

    fun release(leash: L)
}

/** Where a task's surface goes in its parent, and its size, in display pixels. */
data class LeashPlace(val x: Int, val y: Int, val width: Int, val height: Int)

/**
 * The surface (leash) of each organized task. The window manager leaves an organized task's
 * surface position and crop to its organizer, so the agent sets both as the window manager
 * would: at the task's position in its parent, cropped to the task's size.
 *
 * The crop is what the input dispatcher hit-tests by. An app's activity window is touch
 * modal, and the window manager bounds the area it takes pointer input in by its task's
 * surface crop. A task surface with no crop is unbounded, so the topmost task would take
 * every click, wheel turn and hover on the display, at points outside its own window, and
 * the task under the pointer would get none. The crop follows each change of size, so a
 * resized task takes input across its new bounds. A task whose size is not known keeps the
 * crop it has. Visibility stays with the window manager.
 */
class TaskLeashes<L>(private val surfaces: LeashSurfaces<L>) {
    private class Entry<L>(val leash: L, var place: LeashPlace)

    private val tasks = mutableMapOf<Int, Entry<L>>()

    fun appeared(taskId: Int, leash: L, place: LeashPlace) {
        tasks.remove(taskId)?.let { surfaces.release(it.leash) }
        tasks[taskId] = Entry(leash, place)
        surfaces.place(leash, place.x, place.y, place.width, place.height)
    }

    fun changed(taskId: Int, place: LeashPlace) {
        val entry = tasks[taskId] ?: return
        if (entry.place == place) return
        entry.place = place
        surfaces.place(entry.leash, place.x, place.y, place.width, place.height)
    }

    fun vanished(taskId: Int) {
        tasks.remove(taskId)?.let { surfaces.release(it.leash) }
    }
}
