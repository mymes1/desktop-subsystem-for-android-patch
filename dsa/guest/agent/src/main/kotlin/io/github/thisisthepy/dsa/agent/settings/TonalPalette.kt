package io.github.thisisthepy.dsa.agent.settings

import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * The guest's dynamic colors from a seed color: the five tonal palettes Material You apps
 * read (`system_accent1` to `system_accent3`, `system_neutral1` and `system_neutral2`), each
 * in its thirteen shades from 0 (white) to 1000 (black).
 *
 * A phone's SystemUI makes them from the wallpaper or a chosen color. The guest has no
 * SystemUI, so the agent makes them from the host's accent color: the first accent keeps the
 * seed's hue at its saturation, the second the same hue far less saturated, the third the hue
 * turned by 60 degrees, and the neutrals the seed's hue nearly grey. Each shade has the
 * lightness Material's tone of that shade has.
 */
object TonalPalette {
    /** Each shade, with the lightness (in percent) it has. */
    val SHADES = listOf(
        0 to 100.0, 10 to 99.0, 50 to 95.0, 100 to 90.0, 200 to 80.0, 300 to 70.0,
        400 to 60.0, 500 to 49.6, 600 to 40.0, 700 to 30.0, 800 to 20.0, 900 to 10.0,
        1000 to 0.0,
    )

    /** The palette for the seed [argb], by framework color name (such as `system_accent1_500`). */
    fun of(argb: Int): Map<String, Int> {
        val (hue, saturation, _) = hsl(argb)
        // A grey seed still gives a palette with some color in it, as Material's does.
        val chroma = saturation.coerceAtLeast(0.16)
        val palettes = listOf(
            "system_accent1" to Pair(hue, chroma),
            "system_accent2" to Pair(hue, chroma / 3),
            "system_accent3" to Pair((hue + 60) % 360, chroma / 2),
            "system_neutral1" to Pair(hue, 0.04),
            "system_neutral2" to Pair(hue, 0.08),
        )
        val colors = LinkedHashMap<String, Int>()
        for ((name, palette) in palettes) {
            for ((shade, lightness) in SHADES) {
                colors["${name}_$shade"] = fromHsl(palette.first, palette.second, lightness / 100)
            }
        }
        return colors
    }

    /** The hue (0 to 360), saturation and lightness (0 to 1) of [argb]. */
    fun hsl(argb: Int): Triple<Double, Double, Double> {
        val r = (argb shr 16 and 0xff) / 255.0
        val g = (argb shr 8 and 0xff) / 255.0
        val b = (argb and 0xff) / 255.0
        val max = maxOf(r, g, b)
        val min = minOf(r, g, b)
        val lightness = (max + min) / 2
        val delta = max - min
        if (delta == 0.0) return Triple(0.0, 0.0, lightness)
        val saturation = delta / (1 - abs(2 * lightness - 1))
        val hue = when (max) {
            r -> 60 * (((g - b) / delta).mod(6.0))
            g -> 60 * ((b - r) / delta + 2)
            else -> 60 * ((r - g) / delta + 4)
        }
        return Triple(hue, saturation.coerceIn(0.0, 1.0), lightness)
    }

    /** The opaque color of [hue], [saturation] and [lightness]. */
    fun fromHsl(hue: Double, saturation: Double, lightness: Double): Int {
        val c = (1 - abs(2 * lightness - 1)) * saturation
        val x = c * (1 - abs((hue / 60).mod(2.0) - 1))
        val m = lightness - c / 2
        val (r, g, b) = when ((hue / 60).toInt().coerceIn(0, 5)) {
            0 -> Triple(c, x, 0.0)
            1 -> Triple(x, c, 0.0)
            2 -> Triple(0.0, c, x)
            3 -> Triple(0.0, x, c)
            4 -> Triple(x, 0.0, c)
            else -> Triple(c, 0.0, x)
        }
        fun channel(value: Double) = ((value + m) * 255).roundToInt().coerceIn(0, 255)
        return (0xff shl 24) or (channel(r) shl 16) or (channel(g) shl 8) or channel(b)
    }
}
