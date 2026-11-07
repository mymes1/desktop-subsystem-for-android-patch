package io.github.thisisthepy.dsa.agent.input

import android.view.Display
import android.view.InputDevice
import android.view.KeyCharacterMap
import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.control.v1.KeyAction
import io.github.thisisthepy.dsa.control.v1.KeyEvent

/** A key event for the framework to dispatch, as the KeyEvent constructor takes it. */
data class InjectedKey(
    /** When the key went down, in the framework's uptime milliseconds. */
    val downTime: Long,
    /** When this event happened, on the same clock. */
    val eventTime: Long,
    /** KeyEvent.ACTION_DOWN or KeyEvent.ACTION_UP. */
    val action: Int,
    /** KeyEvent.KEYCODE_*. */
    val keyCode: Int,
    val repeatCount: Int,
    /** KeyEvent.META_*. */
    val metaState: Int,
    /** KeyEvent.FLAG_*: FLAG_CANCELED on a release the user did not make. */
    val flags: Int,
    /** Which device the key came from: the framework's full virtual keyboard. */
    val deviceId: Int = KeyCharacterMap.VIRTUAL_KEYBOARD,
    /** InputDevice.SOURCE_*. */
    val source: Int = InputDevice.SOURCE_KEYBOARD,
    /** The display whose focused window receives it. */
    val displayId: Int = Display.DEFAULT_DISPLAY,
)

/**
 * Turns the host's key events into the key events the guest's apps receive, in the task
 * whose window the user typed in.
 *
 * Android sends a key to the focused window, not to a place on the display, so before the
 * first press in a task that does not hold the focus, the task is given it. That is what
 * moving between host windows with the keyboard alone needs: the guest has seen no click to
 * move its focus.
 *
 * The keys come from the framework's virtual keyboard, whose key character map is a full
 * US keyboard, so a key code with its meta state types the character a US layout gives it:
 * letters, digits and punctuation reach a text field as typed. A held key keeps the time it
 * went down, and its repeats and its release are placed after it by the time the host saw
 * them. A release for a key this class never saw go down is dropped, and a release the host
 * marks canceled carries FLAG_CANCELED, so the app lets the key go without acting on it.
 *
 * Esc and Backspace go to the app first, and become Back only when the app leaves them, so a
 * game or an editor that uses them keeps working; on a task's first screen they never do, so
 * a key never closes the window or sends it away. Each takes the framework's own way of
 * telling that an app left a key:
 *
 * - Esc goes as Escape. When the focused window does not take a plain Escape, the framework's
 *   key gesture controller makes it its "close all dialogs" gesture, which only an unhandled
 *   Escape does; the agent hears it ([escapeUnhandled]) and sends Back to the task the Escape
 *   went to, unless that task is on its first screen.
 * - Backspace with no text field focused goes as Backspace with Ctrl and Alt held. The
 *   framework's key character map of its virtual keyboard makes Ctrl+Alt+Backspace that no app
 *   takes into Back (a fallback key), sent to the same window right after the app declined it.
 *   On a task's first screen, and whenever a text field has the focus, Backspace goes as it is,
 *   with no fallback, so it deletes text as before.
 *
 * Not thread safe: the agent calls it from one thread.
 */
class KeyInput(
    private val injector: InputInjector,
    /** Whether the host was told about the task. */
    private val isReported: (Int) -> Boolean,
    /** Whether the task holds the input focus in the guest, as last read. */
    private val isFocused: (Int) -> Boolean,
    /** Gives the task the input focus; false when the framework refused. */
    private val focus: (Int) -> Boolean,
    private val log: Log,
    /** Whether the task is on its first screen (its first activity), where Back closes it. */
    private val onFirstScreen: (Int) -> Boolean = { true },
    /** Whether a text field of the focused window takes typed text now. */
    private val editorFocused: () -> Boolean = { false },
) {
    /** A held key: when it went down, the key code and meta state the guest was given for it. */
    private class Held(val downTime: Long, val hostDownTime: Long, val guestKeyCode: Int, val extraMeta: Int = 0)

    /** The task the last plain Escape went to, and when, until the framework says no app took it. */
    private var escape: Pair<Int, Long>? = null

    /** The keys held in each task, by key code. */
    private val held = HashMap<Int, HashMap<Int, Held>>()
    private val unknownTasks = HashSet<Int>()
    private var failing = false

    /** Acts on one key event from the host. */
    fun handle(event: KeyEvent) {
        val taskId = event.taskId
        if (!isReported(taskId)) {
            if (unknownTasks.add(taskId)) {
                log.warn("dropped key input for task $taskId, which the host was never told about")
            }
            return
        }
        unknownTasks.remove(taskId)
        val keyCode = event.keyCode
        if (keyCode <= 0 || keyCode > MAX_KEY_CODE) {
            log.warn("dropped key input with a key code the guest does not have ($keyCode)")
            return
        }
        val keys = held.getOrPut(taskId) { HashMap() }
        val hostTime = event.eventTimeMs.toLong() and 0xffffffffL
        when (event.action) {
            KeyAction.KEY_ACTION_DOWN -> {
                val known = keys[keyCode]
                val now = injector.now()
                val key = if (known == null) {
                    if (!isFocused(taskId) && !focus(taskId)) {
                        log.warn("could not give task $taskId the input focus; the key may reach another task")
                    }
                    Held(now, hostTime, keyCode, backFallback(taskId, keyCode, event.metaState)).also { keys[keyCode] = it }
                } else {
                    known
                }
                if (known == null && keyCode == android.view.KeyEvent.KEYCODE_ESCAPE) {
                    escape = if (event.metaState and MODIFIERS == 0) taskId to now else null
                }
                inject(
                    InjectedKey(
                        downTime = key.downTime,
                        eventTime = eventTime(key, hostTime, now),
                        action = android.view.KeyEvent.ACTION_DOWN,
                        keyCode = key.guestKeyCode,
                        repeatCount = if (known == null) 0 else event.repeatCount.coerceAtLeast(1),
                        metaState = event.metaState or key.extraMeta,
                        flags = 0,
                    ),
                )
            }
            KeyAction.KEY_ACTION_UP -> {
                val key = keys.remove(keyCode) ?: return
                inject(
                    InjectedKey(
                        downTime = key.downTime,
                        eventTime = eventTime(key, hostTime, injector.now()),
                        action = android.view.KeyEvent.ACTION_UP,
                        keyCode = key.guestKeyCode,
                        repeatCount = 0,
                        metaState = event.metaState or key.extraMeta,
                        flags = if (event.canceled) android.view.KeyEvent.FLAG_CANCELED else 0,
                    ),
                )
            }
            else -> log.warn("dropped key input with an action the agent does not know (${event.actionValue})")
        }
    }

    /**
     * The modifiers a key the host sent goes with besides its own: Ctrl and Alt for a plain
     * Backspace that may become Back (no text field focused, past the task's first screen), so
     * that the framework's fallback makes it Back if the app leaves it. The choice is made when
     * the key goes down and kept until it is released, so the app sees a whole press of one
     * key.
     */
    private fun backFallback(taskId: Int, keyCode: Int, metaState: Int): Int = when {
        keyCode != android.view.KeyEvent.KEYCODE_DEL -> 0
        metaState and MODIFIERS != 0 -> 0
        editorFocused() -> 0
        onFirstScreen(taskId) -> 0
        else -> BACK_FALLBACK
    }

    /**
     * The framework says the focused window took no plain Escape (its key gesture controller
     * made the Escape its close-all-dialogs gesture). The task the last Escape went to goes
     * back, unless it is on its first screen, where Back would close it.
     */
    fun escapeUnhandled() {
        val (taskId, at) = escape ?: return
        escape = null
        if (injector.now() - at > ESCAPE_ANSWER_MILLIS) return
        if (!isReported(taskId) || onFirstScreen(taskId)) return
        pressBack(taskId, android.view.KeyEvent.KEYCODE_BACK)
    }

    /**
     * A mouse's back or forward button went down ([down]) or up over task [taskId]: Back or
     * Forward ([keyCode]) goes to the task as a key, pressed and released with the button.
     */
    fun navigationButton(taskId: Int, keyCode: Int, down: Boolean) {
        handle(
            KeyEvent.newBuilder()
                .setTaskId(taskId)
                .setAction(if (down) KeyAction.KEY_ACTION_DOWN else KeyAction.KEY_ACTION_UP)
                .setKeyCode(keyCode)
                .setEventTimeMs(injector.now().toInt())
                .build(),
        )
    }

    /** Presses and releases [keyCode] in [taskId] at once, as one tap of the key. */
    private fun pressBack(taskId: Int, keyCode: Int) {
        if (!isFocused(taskId) && !focus(taskId)) {
            log.warn("could not give task $taskId the input focus; Back may reach another task")
        }
        val now = injector.now()
        for (action in intArrayOf(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.ACTION_UP)) {
            inject(
                InjectedKey(
                    downTime = now,
                    eventTime = now,
                    action = action,
                    keyCode = keyCode,
                    repeatCount = 0,
                    metaState = 0,
                    flags = 0,
                ),
            )
        }
    }

    /** Forgets the keys held in every task not in [taskIds], as tasks end. */
    fun retainTasks(taskIds: Set<Int>) {
        held.keys.retainAll(taskIds)
        unknownTasks.removeAll(taskIds)
    }

    /**
     * When an event of a held key happened on the guest's clock: as long after the key went
     * down as the host saw it, and never later than now. The host's clock wraps, so only the
     * difference between its times means anything.
     */
    private fun eventTime(key: Held, hostTime: Long, now: Long): Long {
        val since = (hostTime - key.hostDownTime) and 0xffffffffL
        return minOf(key.downTime + since, now).coerceAtLeast(key.downTime)
    }

    private fun inject(key: InjectedKey) {
        val injected = try {
            injector.injectKey(key)
        } catch (e: RuntimeException) {
            if (!failing) log.warn("the framework refused key input", e)
            failing = true
            return
        }
        if (!injected) {
            if (!failing) log.warn("the framework refused key input")
            failing = true
        } else {
            failing = false
        }
    }

    private companion object {
        // The host sends key codes up to this; the framework's own end well below it.
        const val MAX_KEY_CODE = 1023

        // The modifiers that keep Esc and Backspace from ever going back. The locks (Caps, Num,
        // Scroll) do not count.
        const val MODIFIERS = android.view.KeyEvent.META_SHIFT_ON or android.view.KeyEvent.META_CTRL_ON or
            android.view.KeyEvent.META_ALT_ON or android.view.KeyEvent.META_META_ON

        // Ctrl and Alt, left side: the virtual keyboard's key map has Ctrl+Alt+Backspace fall
        // back to Back when no app takes it.
        const val BACK_FALLBACK = android.view.KeyEvent.META_CTRL_ON or android.view.KeyEvent.META_CTRL_LEFT_ON or
            android.view.KeyEvent.META_ALT_ON or android.view.KeyEvent.META_ALT_LEFT_ON

        // How long after an Escape the framework's word that no app took it still counts.
        const val ESCAPE_ANSWER_MILLIS = 1000L
    }
}
