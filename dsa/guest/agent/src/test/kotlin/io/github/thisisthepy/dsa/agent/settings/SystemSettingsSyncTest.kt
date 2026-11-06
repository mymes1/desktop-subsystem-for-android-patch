package io.github.thisisthepy.dsa.agent.settings

import io.github.thisisthepy.dsa.agent.RecordingLog
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.ScreenState
import io.github.thisisthepy.dsa.control.v1.SystemSettings
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SystemSettingsSyncTest {
    private val calls = mutableListOf<String>()
    private var accentPalette: Map<String, Int> = emptyMap()
    private val port = object : SystemSettingsPort {
        override fun setTimeZone(id: String) {
            calls += "time zone $id"
        }

        override fun setLocales(tags: List<String>) {
            calls += "locales $tags"
        }

        override fun setTime24Hour(on: Boolean) {
            calls += "24-hour $on"
        }

        override fun setDarkMode(on: Boolean) {
            calls += "dark $on"
        }

        override fun setAccentColor(argb: Int, palette: Map<String, Int>) {
            calls += "accent %08X".format(argb)
            accentPalette = palette
        }
    }
    private val log = RecordingLog()
    private val sync = SystemSettingsSync(port, log)

    private fun seoul(dark: Boolean = true, accent: Int = 0xFF0078D4.toInt()): ByteArray =
        Envelope.newBuilder()
            .setSystemSettings(
                SystemSettings.newBuilder()
                    .setTimeZone("Asia/Seoul")
                    .addLocales("ko-KR")
                    .addLocales("en-US")
                    .setRegionalFormat("ko-KR")
                    .setTime24Hour(false)
                    .setFirstDayOfWeek(7)
                    .setDarkMode(dark)
                    .setAccentColor(accent),
            )
            .build()
            .toByteArray()

    @Test
    fun fr30_the_hosts_settings_become_the_guests() {
        assertTrue(sync.handle(seoul()))
        assertEquals(
            listOf(
                "time zone Asia/Seoul",
                "locales [ko-KR-u-fw-sun-hc-h12, en-US-u-fw-sun-hc-h12-rg-krzzzz]",
                "24-hour false",
                "dark true",
                "accent FF0078D4",
            ),
            calls,
        )
        assertEquals(5 * 13, accentPalette.size)
    }

    @Test
    fun fr30_only_what_changed_is_applied_again() {
        sync.handle(seoul())
        calls.clear()
        sync.handle(seoul(dark = false))
        assertEquals(listOf("dark false"), calls)
        calls.clear()
        sync.handle(seoul(dark = false, accent = 0xFFE81123.toInt()))
        assertEquals(listOf("accent FFE81123"), calls)
        // A new connection applies everything again.
        calls.clear()
        sync.connected()
        sync.handle(seoul(dark = false, accent = 0xFFE81123.toInt()))
        assertEquals(5, calls.size)
    }

    @Test
    fun fr30_a_setting_the_guest_refuses_does_not_keep_the_others_from_applying() {
        val refusing = object : SystemSettingsPort by SystemSettingsPort.NONE {
            override fun setTimeZone(id: String) = throw SecurityException("no")

            override fun setDarkMode(on: Boolean) {
                calls += "dark $on"
            }
        }
        val sync = SystemSettingsSync(refusing, log)
        sync.handle(seoul())
        assertEquals(listOf("dark true"), calls)
        assertTrue(log.lines.any { it.startsWith("warn: could not follow the host's time zone") })
    }

    @Test
    fun fr30_other_frames_are_not_settings() {
        val screen = Envelope.newBuilder().setScreenState(ScreenState.newBuilder().setOn(true)).build()
        assertFalse(sync.handle(screen.toByteArray()))
        assertFalse(sync.handle(byteArrayOf(1, 2, 3)))
        assertTrue(calls.isEmpty())
    }

    @Test
    fun fr30_the_accent_color_seeds_a_palette_from_white_to_black() {
        val palette = TonalPalette.of(0xFF0078D4.toInt())
        assertEquals(0xFFFFFFFF.toInt(), palette["system_accent1_0"])
        assertEquals(0xFF000000.toInt(), palette["system_accent1_1000"])
        val (hue, _, _) = TonalPalette.hsl(palette.getValue("system_accent1_500"))
        val (seedHue, _, _) = TonalPalette.hsl(0xFF0078D4.toInt())
        assertEquals(seedHue, hue, 3.0)
        // The neutrals are nearly grey.
        val (_, saturation, _) = TonalPalette.hsl(palette.getValue("system_neutral1_500"))
        assertTrue(saturation < 0.1)
    }
}
