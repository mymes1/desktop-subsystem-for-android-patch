package io.github.thisisthepy.dsa.agent.share

/**
 * What an app shared through "Share on PC", as the share intent gave it: its text and links,
 * its files (content URIs, of any type the platform uses for them), and its subject.
 */
data class SharedContent<U>(
    /** The subject or title the app gave; empty for none. */
    val title: String,
    /** The text and links, without repeats, in order. */
    val texts: List<String>,
    /** The files, without repeats, in order. */
    val files: List<U>,
    /** The MIME type the intent named, used for files whose provider names none. */
    val type: String?,
) {
    val isEmpty: Boolean get() = texts.isEmpty() && files.isEmpty()

    companion object {
        const val ACTION_SEND = "android.intent.action.SEND"
        const val ACTION_SEND_MULTIPLE = "android.intent.action.SEND_MULTIPLE"

        /** The most items one share carries, as the host takes. */
        const val MAX_ITEMS = 100

        /**
         * The content of a share intent of [action], or null when it is not a share. Blank text
         * is dropped, and a type with a wildcard ("image/ *") names no file's type.
         */
        fun <U> from(action: String?, type: String?, texts: List<String>, files: List<U>, subject: String?): SharedContent<U>? {
            if (action != ACTION_SEND && action != ACTION_SEND_MULTIPLE) return null
            val kept = texts.map { it.trim() }.filter { it.isNotEmpty() }.distinct()
            val unique = files.distinct()
            val room = (MAX_ITEMS - kept.size).coerceAtLeast(0)
            return SharedContent(
                title = subject?.trim().orEmpty(),
                texts = kept.take(MAX_ITEMS),
                files = unique.take(room),
                type = type?.takeIf { '*' !in it },
            )
        }
    }
}
