package io.github.thisisthepy.dsa.agent.tasks

/**
 * Where an adaptive icon's layer is drawn so that it comes out at the scale of the whole icon.
 *
 * AdaptiveIconDrawable draws its layers a quarter of the icon's size larger on every side than
 * the icon itself (its EXTRA_INSET_PERCENTAGE): a layer is 108 dp for a 72 dp icon, and the
 * mask shows its middle. The foreground drawn alone into the same square, at the same bounds,
 * keeps the size and place it has in the full icon, so the title bar's icon is the app's
 * shape at the size the taskbar's icon shows it, without the plate behind it.
 */
object CaptionIcon {
    /** A rectangle in pixels: left and top inclusive, right and bottom exclusive. */
    data class Bounds(val left: Int, val top: Int, val right: Int, val bottom: Int)

    /** The bounds a layer is drawn at in a square icon of [size] pixels. */
    fun layerBounds(size: Int): Bounds {
        val inset = size / 4
        return Bounds(-inset, -inset, size + inset, size + inset)
    }
}
