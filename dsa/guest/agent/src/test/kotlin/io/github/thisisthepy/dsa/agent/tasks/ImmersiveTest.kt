package io.github.thisisthepy.dsa.agent.tasks

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ImmersiveTest {
    @Test
    fun fr30_an_app_that_hides_both_its_bars_fills_the_screen() {
        // Only the IME and the caption bar are asked to be visible.
        assertTrue(hidesBothSystemBars(8 or 4))
        assertTrue(hidesBothSystemBars(0))
    }

    @Test
    fun fr30_an_app_that_keeps_either_bar_or_says_nothing_does_not() {
        assertFalse(hidesBothSystemBars(STATUS_BARS))
        assertFalse(hidesBothSystemBars(NAVIGATION_BARS or 8))
        assertFalse(hidesBothSystemBars(STATUS_BARS or NAVIGATION_BARS))
        assertFalse(hidesBothSystemBars(null))
    }
}
