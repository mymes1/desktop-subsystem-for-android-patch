package io.github.thisisthepy.dsa.agent.settings

import com.google.protobuf.InvalidProtocolBufferException
import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.control.v1.Envelope
import io.github.thisisthepy.dsa.control.v1.SystemSettings

/**
 * What the agent changes in the guest's system settings. The platform's is
 * [io.github.thisisthepy.dsa.agent.platform.AndroidSystemSettings]; a test's records the calls.
 */
interface SystemSettingsPort {
    /** Makes [id], an IANA time zone name, the guest's time zone. */
    fun setTimeZone(id: String)

    /** Makes [tags], BCP 47 language tags preferred first, the guest's system languages. */
    fun setLocales(tags: List<String>)

    /** Shows times on a 24-hour clock, or a 12-hour one. */
    fun setTime24Hour(on: Boolean)

    /** Turns the guest's dark theme on or off. */
    fun setDarkMode(on: Boolean)

    /** Makes [argb] the seed of the guest's dynamic colors, as [palette] says them. */
    fun setAccentColor(argb: Int, palette: Map<String, Int>)

    /** The guest's font scale now (Settings.System.FONT_SCALE); 0 when it cannot be read. */
    fun fontScale(): Float

    /** Makes [scale] the guest's font scale, which every app's text is drawn at. */
    fun setFontScale(scale: Float)

    companion object {
        val NONE = object : SystemSettingsPort {
            override fun setTimeZone(id: String) {}
            override fun setLocales(tags: List<String>) {}
            override fun setTime24Hour(on: Boolean) {}
            override fun setDarkMode(on: Boolean) {}
            override fun setAccentColor(argb: Int, palette: Map<String, Int>) {}
            override fun fontScale(): Float = 0f
            override fun setFontScale(scale: Float) {}
        }
    }
}

/**
 * Makes the host's system settings the guest's: the time zone, the display languages with the
 * regional format's preferences, the 24-hour clock, the dark theme and the accent color.
 *
 * The host sends every setting each time ([SystemSettings]); only what changed since the last
 * time is applied, so a change of theme does not reconfigure every app for its languages too.
 * Each setting is applied on its own: one the guest refuses is logged, and the others still
 * apply. An empty time zone or language list leaves the guest's as they are.
 *
 * The font scale keeps text the size of the host's text ([SystemSettings.getFontScale]). A
 * change of font scale is a change of configuration, and Android restarts every activity that
 * does not say in its manifest that it handles one (configChanges `fontScale`), as it does for
 * a change of density. No app is restarted for it: while an app is open that would be
 * ([tasksRunning]), the new scale is held, and it applies as soon as none is. The first scale
 * after the agent starts applies before any app opens, so apps start at it.
 */
class SystemSettingsSync(
    private val port: SystemSettingsPort,
    private val log: Log,
) {
    private var last: SystemSettings? = null

    /** The font scale the host asked for that is not applied yet. */
    private var heldFontScale: Float? = null

    /** The open apps a change of font scale would restart; null until the tasks are read. */
    private var restarted: List<String>? = null

    /** The apps the held font scale was last logged as waiting for. */
    private var loggedWaiting: List<String>? = null

    /**
     * The open apps whose activity does not handle a change of font scale itself, so Android
     * would restart them for one, as the agent last read the tasks. A held font scale applies
     * once there are none.
     */
    fun tasksRunning(wouldRestart: List<String>) {
        restarted = wouldRestart
        applyFontScale()
    }

    /** Whether a font scale the host asked for is waiting for an app to close. */
    val holdsFontScale: Boolean
        get() = heldFontScale != null

    /** Acts on [frame] when it carries the host's system settings, and says whether it did. */
    fun handle(frame: ByteArray): Boolean {
        val envelope = try {
            Envelope.parseFrom(frame)
        } catch (e: InvalidProtocolBufferException) {
            return false
        }
        if (!envelope.hasSystemSettings()) return false
        apply(envelope.systemSettings)
        return true
    }

    /** Forgets what was applied, for a new connection: the next settings apply in full. */
    fun connected() {
        last = null
    }

    private fun apply(settings: SystemSettings) {
        val before = last
        if (settings.timeZone.isNotEmpty() && settings.timeZone != before?.timeZone) {
            attempt("time zone ${settings.timeZone}") { port.setTimeZone(settings.timeZone) }
        }
        val tags = localeTags(settings)
        if (tags.isNotEmpty() && (before == null || tags != localeTags(before))) {
            attempt("languages $tags") { port.setLocales(tags) }
        }
        if (before == null || settings.time24Hour != before.time24Hour) {
            attempt("the ${if (settings.time24Hour) "24" else "12"}-hour clock") {
                port.setTime24Hour(settings.time24Hour)
            }
        }
        if (before == null || settings.darkMode != before.darkMode) {
            attempt("the ${if (settings.darkMode) "dark" else "light"} theme") {
                port.setDarkMode(settings.darkMode)
            }
        }
        val accent = settings.accentColor
        if (accent != 0 && (before == null || accent != before.accentColor)) {
            attempt("accent color ${"%08X".format(accent)}") {
                port.setAccentColor(accent, TonalPalette.of(accent))
            }
        }
        val scale = settings.fontScale
        if (scale.isFinite() && scale > 0f && (before == null || scale != before.fontScale)) {
            heldFontScale = scale.coerceIn(MIN_FONT_SCALE, MAX_FONT_SCALE)
            loggedWaiting = null
            applyFontScale()
        }
        last = settings
    }

    /** Applies the held font scale, unless an open app would be restarted for it. */
    private fun applyFontScale() {
        val wanted = heldFontScale ?: return
        val now = try {
            port.fontScale()
        } catch (e: Exception) {
            0f
        }
        if (kotlin.math.abs(now - wanted) < SAME_FONT_SCALE) {
            heldFontScale = null
            return
        }
        // Until the tasks are read, an app that is open is not known to be safe.
        val waiting = restarted ?: return
        if (waiting.isNotEmpty()) {
            if (waiting != loggedWaiting) {
                log.info(
                    "holds the host's text size (font scale ${scaleText(wanted)}) while " +
                        "${waiting.joinToString()} ${if (waiting.size == 1) "is" else "are"} open: " +
                        "Android would restart ${if (waiting.size == 1) "it" else "them"} for it",
                )
                loggedWaiting = waiting
            }
            return
        }
        heldFontScale = null
        loggedWaiting = null
        attempt("text size (font scale ${scaleText(wanted)})") { port.setFontScale(wanted) }
    }

    private fun attempt(what: String, change: () -> Unit) {
        try {
            change()
            log.info("follows the host's $what")
        } catch (e: Exception) {
            log.warn("could not follow the host's $what", e)
        }
    }

    companion object {
        private val WEEKDAYS = listOf("mon", "tue", "wed", "thu", "fri", "sat", "sun")

        /** The font scales Android's settings provider takes for Settings.System.FONT_SCALE. */
        const val MIN_FONT_SCALE = 0.25f
        const val MAX_FONT_SCALE = 5.0f

        /** Two font scales closer than this draw text at the same size. */
        private const val SAME_FONT_SCALE = 0.0005f

        /** A font scale for the log, the same in every locale. */
        private fun scaleText(scale: Float): String = String.format(java.util.Locale.ROOT, "%.3f", scale)

        /**
         * The guest's system languages for [settings]: the host's languages, each carrying the
         * regional format's preferences as Unicode extensions, which Android's regional
         * preferences read: the first day of the week (`fw`), the hour cycle (`hc`), and the
         * region the format is of when it is another than the language's (`rg`).
         */
        fun localeTags(settings: SystemSettings): List<String> {
            val extensions = mutableListOf<String>()
            val day = settings.firstDayOfWeek
            if (day in 1..7) extensions += "fw-${WEEKDAYS[day - 1]}"
            extensions += if (settings.time24Hour) "hc-h23" else "hc-h12"
            val formatRegion = region(settings.regionalFormat)
            return settings.localesList.filter { it.isNotBlank() }.map { tag ->
                val all = extensions.toMutableList()
                if (formatRegion != null && formatRegion != region(tag)) {
                    all += "rg-${formatRegion.lowercase()}zzzz"
                }
                // An extension the host's tag carries already is left as it is.
                if ("-u-" in tag) tag else "$tag-u-${all.joinToString("-")}"
            }
        }

        /** The region of a BCP 47 tag (its two-letter or three-digit part), if it has one. */
        private fun region(tag: String): String? =
            tag.split('-').drop(1).firstOrNull { part ->
                (part.length == 2 && part.all { it.isLetter() }) || (part.length == 3 && part.all { it.isDigit() })
            }?.uppercase()
    }
}
