package io.github.thisisthepy.dsa.agent.tasks

import io.github.thisisthepy.dsa.agent.AGENT
import io.github.thisisthepy.dsa.agent.appTask
import io.github.thisisthepy.dsa.agent.decode
import io.github.thisisthepy.dsa.control.v1.Envelope
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CaptionIconTest {
    private val sent = mutableListOf<Envelope>()

    private fun announce(details: AppDetails): Envelope {
        sent.clear()
        val reporter = TaskReporter(AGENT, { details }) { frame ->
            sent += decode(frame)
            true
        }
        reporter.update(listOf(appTask(1, packageName = "org.example.mail")))
        return sent.first { it.bodyCase == Envelope.BodyCase.APP_INFO }
    }

    @Test
    fun fr22_the_title_bar_icon_is_the_adaptive_foreground() {
        // An adaptive icon: the taskbar's icon keeps its plate, the title bar's is the
        // foreground alone.
        val adaptive = announce(AppDetails("Mail", byteArrayOf(1, 2, 3), byteArrayOf(4, 5)))
        assertArrayEquals(byteArrayOf(1, 2, 3), adaptive.appInfo.iconPng.toByteArray())
        assertArrayEquals(byteArrayOf(4, 5), adaptive.appInfo.captionIconPng.toByteArray())
        // An icon that is not adaptive has no foreground: the title bar shows the whole icon.
        val plain = announce(AppDetails("Mail", byteArrayOf(1, 2, 3)))
        assertTrue(plain.appInfo.captionIconPng.isEmpty)
        assertArrayEquals(byteArrayOf(1, 2, 3), plain.appInfo.iconPng.toByteArray())
    }

    @Test
    fun fr22_the_foreground_is_drawn_at_the_scale_of_the_whole_icon() {
        // A layer is a quarter of the icon larger on every side: 108 dp for a 72 dp icon.
        assertEquals(CaptionIcon.Bounds(-128, -128, 640, 640), CaptionIcon.layerBounds(512))
        val bounds = CaptionIcon.layerBounds(256)
        assertEquals(256 * 108 / 72, bounds.right - bounds.left)
        assertEquals(bounds.left, -(bounds.right - 256))
    }

    @Test
    fun fr22_an_app_that_supports_several_windows_says_so() {
        assertTrue(announce(AppDetails("Mail", byteArrayOf(1), multipleWindows = true)).appInfo.multipleWindows)
        assertFalse(announce(AppDetails("Mail", byteArrayOf(1))).appInfo.multipleWindows)
    }
}
