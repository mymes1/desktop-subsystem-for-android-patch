package io.github.thisisthepy.dsa.agent.input

import android.view.Display
import android.view.InputDevice
import android.view.KeyCharacterMap
import io.github.thisisthepy.dsa.agent.FakeInjector
import io.github.thisisthepy.dsa.agent.RecordingLog
import io.github.thisisthepy.dsa.control.v1.KeyAction
import io.github.thisisthepy.dsa.control.v1.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import android.view.KeyEvent as AndroidKey

class KeyInputTest {
    private val injector = FakeInjector()
    private val log = RecordingLog()
    private val reported = mutableSetOf(7, 8)
    private val focusedTasks = mutableSetOf(7)
    private val focusRequests = mutableListOf<Int>()
    private var focusWorks = true
    private val pastFirstScreen = mutableSetOf<Int>()
    private var editing = false
    private val input = KeyInput(
        injector,
        { it in reported },
        { it in focusedTasks },
        { taskId ->
            focusRequests += taskId
            focusWorks
        },
        log,
        onFirstScreen = { it !in pastFirstScreen },
        editorFocused = { editing },
    )

    private fun event(
        action: KeyAction,
        keyCode: Int,
        meta: Int = 0,
        repeat: Int = 0,
        time: Int = 0,
        canceled: Boolean = false,
        taskId: Int = 7,
    ): KeyEvent = KeyEvent.newBuilder()
        .setTaskId(taskId)
        .setAction(action)
        .setKeyCode(keyCode)
        .setMetaState(meta)
        .setRepeatCount(repeat)
        .setEventTimeMs(time)
        .setCanceled(canceled)
        .build()

    private fun down(keyCode: Int, meta: Int = 0, repeat: Int = 0, time: Int = 0, taskId: Int = 7) =
        input.handle(event(KeyAction.KEY_ACTION_DOWN, keyCode, meta, repeat, time, taskId = taskId))

    private fun up(keyCode: Int, meta: Int = 0, time: Int = 0, canceled: Boolean = false, taskId: Int = 7) =
        input.handle(event(KeyAction.KEY_ACTION_UP, keyCode, meta, time = time, canceled = canceled, taskId = taskId))

    @Test
    fun fr16_a_key_press_is_a_keyboard_down_and_up_on_the_default_display() {
        down(AndroidKey.KEYCODE_ENTER)
        up(AndroidKey.KEYCODE_ENTER)

        assertEquals(listOf(AndroidKey.ACTION_DOWN, AndroidKey.ACTION_UP), injector.keys.map { it.action })
        for (key in injector.keys) {
            assertEquals(AndroidKey.KEYCODE_ENTER, key.keyCode)
            assertEquals(InputDevice.SOURCE_KEYBOARD, key.source)
            assertEquals(KeyCharacterMap.VIRTUAL_KEYBOARD, key.deviceId)
            assertEquals(Display.DEFAULT_DISPLAY, key.displayId)
            assertEquals(0, key.flags)
        }
    }

    @Test
    fun fr5_esc_goes_to_the_app_first_as_escape() {
        pastFirstScreen += 7
        down(AndroidKey.KEYCODE_ESCAPE)
        up(AndroidKey.KEYCODE_ESCAPE)

        assertEquals(listOf(AndroidKey.ACTION_DOWN, AndroidKey.ACTION_UP), injector.keys.map { it.action })
        assertEquals(listOf(AndroidKey.KEYCODE_ESCAPE, AndroidKey.KEYCODE_ESCAPE), injector.keys.map { it.keyCode })
        assertEquals(listOf(0, 0), injector.keys.map { it.metaState })
    }

    @Test
    fun fr5_an_esc_the_app_leaves_goes_back() {
        pastFirstScreen += 7
        down(AndroidKey.KEYCODE_ESCAPE)
        input.escapeUnhandled()
        up(AndroidKey.KEYCODE_ESCAPE)

        assertEquals(
            listOf(AndroidKey.KEYCODE_ESCAPE, AndroidKey.KEYCODE_BACK, AndroidKey.KEYCODE_BACK, AndroidKey.KEYCODE_ESCAPE),
            injector.keys.map { it.keyCode },
        )
        assertEquals(AndroidKey.ACTION_DOWN, injector.keys[1].action)
        assertEquals(AndroidKey.ACTION_UP, injector.keys[2].action)
        // The framework's word counts once: a second one is about no Escape.
        input.escapeUnhandled()
        assertEquals(4, injector.keys.size)
    }

    @Test
    fun fr5_an_esc_the_app_takes_does_not_go_back() {
        pastFirstScreen += 7
        down(AndroidKey.KEYCODE_ESCAPE)
        up(AndroidKey.KEYCODE_ESCAPE)
        // No word from the framework: the app took it. A late word about it counts for nothing.
        injector.clock += 5_000
        input.escapeUnhandled()

        assertEquals(listOf(AndroidKey.KEYCODE_ESCAPE, AndroidKey.KEYCODE_ESCAPE), injector.keys.map { it.keyCode })
    }

    @Test
    fun fr5_an_esc_on_the_tasks_first_screen_does_nothing_more() {
        down(AndroidKey.KEYCODE_ESCAPE)
        input.escapeUnhandled()
        up(AndroidKey.KEYCODE_ESCAPE)

        assertEquals(listOf(AndroidKey.KEYCODE_ESCAPE, AndroidKey.KEYCODE_ESCAPE), injector.keys.map { it.keyCode })
    }

    @Test
    fun fr5_esc_with_a_modifier_stays_escape_and_never_goes_back() {
        pastFirstScreen += 7
        val shift = AndroidKey.META_SHIFT_ON or AndroidKey.META_SHIFT_LEFT_ON
        down(AndroidKey.KEYCODE_ESCAPE, shift)
        input.escapeUnhandled()
        up(AndroidKey.KEYCODE_ESCAPE, shift)

        assertEquals(listOf(AndroidKey.KEYCODE_ESCAPE, AndroidKey.KEYCODE_ESCAPE), injector.keys.map { it.keyCode })
    }

    @Test
    fun fr5_backspace_with_no_text_field_falls_back_to_back_when_the_app_leaves_it() {
        pastFirstScreen += 7
        down(AndroidKey.KEYCODE_DEL)
        down(AndroidKey.KEYCODE_DEL, repeat = 1, time = 500)
        up(AndroidKey.KEYCODE_DEL)

        // The app gets Backspace first; held with Ctrl and Alt, the framework's key map turns
        // one no app takes into Back. The whole press carries them.
        val fallback = AndroidKey.META_CTRL_ON or AndroidKey.META_CTRL_LEFT_ON or
            AndroidKey.META_ALT_ON or AndroidKey.META_ALT_LEFT_ON
        assertEquals(List(3) { AndroidKey.KEYCODE_DEL }, injector.keys.map { it.keyCode })
        assertEquals(List(3) { fallback }, injector.keys.map { it.metaState })
    }

    @Test
    fun fr5_backspace_in_a_text_field_deletes_text_as_it_is() {
        pastFirstScreen += 7
        editing = true
        down(AndroidKey.KEYCODE_DEL)
        up(AndroidKey.KEYCODE_DEL)

        assertEquals(listOf(AndroidKey.KEYCODE_DEL, AndroidKey.KEYCODE_DEL), injector.keys.map { it.keyCode })
        assertEquals(listOf(0, 0), injector.keys.map { it.metaState })
    }

    @Test
    fun fr5_backspace_on_the_tasks_first_screen_never_goes_back() {
        down(AndroidKey.KEYCODE_DEL)
        up(AndroidKey.KEYCODE_DEL)
        val ctrl = AndroidKey.META_CTRL_ON or AndroidKey.META_CTRL_LEFT_ON
        pastFirstScreen += 7
        down(AndroidKey.KEYCODE_DEL, ctrl)
        up(AndroidKey.KEYCODE_DEL, ctrl)

        // Plain on the first screen; with a modifier it is the app's shortcut, as it was sent.
        assertEquals(listOf(0, 0, ctrl, ctrl), injector.keys.map { it.metaState })
    }

    @Test
    fun fr5_the_mouse_back_and_forward_buttons_are_back_and_forward_keys() {
        input.navigationButton(8, AndroidKey.KEYCODE_BACK, true)
        input.navigationButton(8, AndroidKey.KEYCODE_BACK, false)
        input.navigationButton(8, AndroidKey.KEYCODE_FORWARD, true)
        input.navigationButton(8, AndroidKey.KEYCODE_FORWARD, false)

        assertEquals(
            listOf(AndroidKey.KEYCODE_BACK, AndroidKey.KEYCODE_BACK, AndroidKey.KEYCODE_FORWARD, AndroidKey.KEYCODE_FORWARD),
            injector.keys.map { it.keyCode },
        )
        assertEquals(
            listOf(AndroidKey.ACTION_DOWN, AndroidKey.ACTION_UP, AndroidKey.ACTION_DOWN, AndroidKey.ACTION_UP),
            injector.keys.map { it.action },
        )
        // The task under the pointer takes the focus, so the key reaches it.
        assertEquals(listOf(8), focusRequests.distinct())
    }

    @Test
    fun fr16_ctrl_a_carries_the_meta_state_the_host_sent() {
        val ctrl = AndroidKey.META_CTRL_ON or AndroidKey.META_CTRL_LEFT_ON
        down(AndroidKey.KEYCODE_CTRL_LEFT, ctrl)
        down(AndroidKey.KEYCODE_A, ctrl)
        up(AndroidKey.KEYCODE_A, ctrl)
        up(AndroidKey.KEYCODE_CTRL_LEFT, 0)

        assertEquals(
            listOf(
                AndroidKey.KEYCODE_CTRL_LEFT to ctrl,
                AndroidKey.KEYCODE_A to ctrl,
                AndroidKey.KEYCODE_A to ctrl,
                AndroidKey.KEYCODE_CTRL_LEFT to 0,
            ),
            injector.keys.map { it.keyCode to it.metaState },
        )
    }

    @Test
    fun fr16_typed_letters_digits_and_punctuation_reach_the_task_with_shift() {
        val shift = AndroidKey.META_SHIFT_ON or AndroidKey.META_SHIFT_LEFT_ON
        for (code in listOf(AndroidKey.KEYCODE_H, AndroidKey.KEYCODE_1, AndroidKey.KEYCODE_SLASH)) {
            down(code, shift)
            up(code, shift)
        }
        val downs = injector.keys.filter { it.action == AndroidKey.ACTION_DOWN }
        assertEquals(listOf(AndroidKey.KEYCODE_H, AndroidKey.KEYCODE_1, AndroidKey.KEYCODE_SLASH), downs.map { it.keyCode })
        assertTrue(downs.all { it.metaState == shift })
    }

    @Test
    fun fr16_a_held_key_repeats_from_the_time_it_went_down() {
        injector.clock = 5000
        down(AndroidKey.KEYCODE_DEL, time = 100)
        injector.clock = 5600
        down(AndroidKey.KEYCODE_DEL, repeat = 1, time = 600)
        injector.clock = 5700
        down(AndroidKey.KEYCODE_DEL, repeat = 2, time = 650)
        up(AndroidKey.KEYCODE_DEL, time = 700)

        assertEquals(listOf(0, 1, 2, 0), injector.keys.map { it.repeatCount })
        assertTrue(injector.keys.all { it.downTime == 5000L })
        assertEquals(listOf(5000L, 5500L, 5550L, 5600L), injector.keys.map { it.eventTime })
    }

    @Test
    fun fr16_the_hosts_clock_wrapping_does_not_move_an_event_before_its_press() {
        injector.clock = 9000
        down(AndroidKey.KEYCODE_A, time = -10)
        injector.clock = 9030
        up(AndroidKey.KEYCODE_A, time = 15)

        assertEquals(9025L, injector.keys.last().eventTime)
    }

    @Test
    fun fr16_a_canceled_release_tells_the_app_not_to_act_on_the_key() {
        down(AndroidKey.KEYCODE_ENTER)
        up(AndroidKey.KEYCODE_ENTER, canceled = true)

        assertEquals(AndroidKey.FLAG_CANCELED, injector.keys.last().flags)
        assertEquals(AndroidKey.ACTION_UP, injector.keys.last().action)
    }

    @Test
    fun fr16_a_release_of_a_key_never_pressed_is_dropped() {
        up(AndroidKey.KEYCODE_TAB)

        assertTrue(injector.keys.isEmpty())
    }

    @Test
    fun fr16_a_key_for_a_task_without_the_focus_focuses_it_first() {
        down(AndroidKey.KEYCODE_A, taskId = 8)
        down(AndroidKey.KEYCODE_A, repeat = 1, taskId = 8)
        up(AndroidKey.KEYCODE_A, taskId = 8)
        // The focused task is not focused again.
        down(AndroidKey.KEYCODE_B, taskId = 7)

        assertEquals(listOf(8), focusRequests)
        assertEquals(4, injector.keys.size)
    }

    @Test
    fun fr16_a_key_still_goes_in_when_the_focus_is_refused() {
        focusWorks = false
        down(AndroidKey.KEYCODE_A, taskId = 8)

        assertEquals(1, injector.keys.size)
        assertTrue(log.lines.any { it.startsWith("warn: could not give task 8 the input focus") })
    }

    @Test
    fun fr16_a_key_for_a_task_the_host_was_not_told_about_is_dropped() {
        down(AndroidKey.KEYCODE_A, taskId = 99)
        down(AndroidKey.KEYCODE_B, taskId = 99)

        assertTrue(injector.keys.isEmpty())
        assertEquals(1, log.lines.count { it.contains("task 99") })
    }

    @Test
    fun fr16_keys_held_in_a_task_that_ended_are_forgotten() {
        down(AndroidKey.KEYCODE_SHIFT_LEFT)
        input.retainTasks(setOf(8))
        reported.remove(7)
        reported.add(7)
        up(AndroidKey.KEYCODE_SHIFT_LEFT)

        assertEquals(listOf(AndroidKey.ACTION_DOWN), injector.keys.map { it.action })
    }

    @Test
    fun nfr5_a_refused_key_is_logged_once_and_input_goes_on() {
        injector.refuse = true
        down(AndroidKey.KEYCODE_A)
        up(AndroidKey.KEYCODE_A)
        injector.refuse = false
        down(AndroidKey.KEYCODE_B)

        assertEquals(1, log.lines.count { it.startsWith("warn: the framework refused key input") })
        assertEquals(listOf(AndroidKey.KEYCODE_B), injector.keys.map { it.keyCode })
    }
}
