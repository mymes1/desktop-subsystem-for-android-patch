package io.github.thisisthepy.dsa.agent.tasks

/** An app's own name and launcher icon, as the host's window for the app shows them. */
class AppDetails(
    /** The app's name, as a launcher shows it. */
    val label: String,
    /** The launcher icon as a square PNG image; empty when the app has none to draw. */
    val iconPng: ByteArray,
    /**
     * The icon a window's title bar shows: an adaptive icon's foreground layer alone, at the
     * same size and scale as [iconPng], so no square plate stands over the app. Empty for an
     * app whose icon is not adaptive, which shows [iconPng] there too.
     */
    val captionIconPng: ByteArray = ByteArray(0),
    /** Whether the app supports several windows at once, so its windows offer New window. */
    val multipleWindows: Boolean = false,
)

/** Looks up the name and icon of an installed app. */
fun interface AppInfoSource {
    /** The details of [packageName], or null when the package is not installed. */
    fun appInfo(packageName: String): AppDetails?

    companion object {
        /** Knows no app, so no app's details are sent. */
        val NONE = AppInfoSource { null }
    }
}
