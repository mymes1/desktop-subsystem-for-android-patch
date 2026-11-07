package io.github.thisisthepy.dsa.agent.input

/**
 * Says when the focused window took no plain Escape: the framework's key gesture controller
 * makes such an Escape its "close all dialogs" gesture, after the app declined it.
 */
fun interface UnhandledKeys {
    /** Calls [onEscape], on any thread, each time an Escape no app took reaches the system. */
    fun listen(onEscape: () -> Unit): Boolean

    companion object {
        /** Says nothing, so Esc never goes back. */
        val NONE = UnhandledKeys { false }
    }
}
