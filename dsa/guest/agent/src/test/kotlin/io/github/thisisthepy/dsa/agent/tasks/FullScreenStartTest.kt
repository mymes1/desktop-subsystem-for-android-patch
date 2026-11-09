package io.github.thisisthepy.dsa.agent.tasks

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FullScreenStartTest {
    @Test
    fun fr30_an_app_whose_theme_is_full_screen_fills_the_screen_before_it_has_a_window() {
        // Before the activity adds its window, the task asks for every bar (the framework's
        // default); the theme alone says it will fill the screen.
        assertTrue(asksForFullScreen(fullscreenMode = false, requestedVisibleTypes = 503, fullscreenTheme = true))
        assertTrue(asksForFullScreen(fullscreenMode = false, requestedVisibleTypes = null, fullscreenTheme = true))
        // The other ways still count.
        assertTrue(asksForFullScreen(fullscreenMode = true, requestedVisibleTypes = 503, fullscreenTheme = false))
        assertTrue(asksForFullScreen(fullscreenMode = false, requestedVisibleTypes = 368, fullscreenTheme = false))
        assertFalse(asksForFullScreen(fullscreenMode = false, requestedVisibleTypes = 503, fullscreenTheme = false))
    }

    @Test
    fun fr4_an_app_whose_theme_is_full_screen_starts_at_the_whole_display_so_it_is_not_relaunched() {
        // hello-vulkan sets android:windowFullscreen and hides its bars in onCreate. Started at
        // the framework's 412 by 732 default, its window filled the 1920 by 1080 monitor right
        // after the activity was created, which changed its smallest width and screen layout
        // and relaunched it. It starts at the whole display instead.
        assertEquals(
            TaskBounds(0, 0, 1920, 1080),
            launchBounds(fullscreenTheme = true, displayWidth = 1920, displayHeight = 1080),
        )
        assertEquals(
            TaskBounds(0, 0, 2560, 2560),
            launchBounds(fullscreenTheme = true, displayWidth = 2560, displayHeight = 2560),
        )
        // Any other app keeps the framework's own default.
        assertNull(launchBounds(fullscreenTheme = false, displayWidth = 1920, displayHeight = 1080))
        // A display whose size is not known gives no bounds.
        assertNull(launchBounds(fullscreenTheme = true, displayWidth = 0, displayHeight = 0))
    }
}
