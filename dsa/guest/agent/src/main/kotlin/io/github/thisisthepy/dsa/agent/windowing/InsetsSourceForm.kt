package io.github.thisisthepy.dsa.agent.windowing

/**
 * Which form of WindowContainerTransaction.addInsetsSource to call, read from the forms this
 * framework has (each as its parameter type names).
 *
 * Android 16 has two forms with seven parameters: the frame in display pixels (Rect) and the
 * size relative to the task's edges (Insets). Picking a form by its parameter count alone
 * found the Insets one and passed it a Rect, which the framework refused, so no task got a
 * top inset and every app drew its toolbar under the host's caption buttons. A frame form is
 * preferred; the Insets form is used when it is the only one.
 */
sealed interface InsetsSourceForm {
    /** The index of this form in the list it was chosen from. */
    val index: Int

    /** addInsetsSource(token, owner, index, type, Rect frame[, Rect[] bounding[, int flags]]). */
    data class Frame(override val index: Int, val parameterCount: Int) : InsetsSourceForm

    /** addInsetsSource(token, owner, index, type, Insets size[, Rect[] bounding[, int flags]]). */
    data class Size(override val index: Int, val parameterCount: Int) : InsetsSourceForm

    companion object {
        private const val RECT = "android.graphics.Rect"
        private const val INSETS = "android.graphics.Insets"

        /** The form to call among [forms], or null when none takes a frame or a size. */
        fun choose(forms: List<List<String>>): InsetsSourceForm? {
            fun candidates(kind: String) = forms.withIndex()
                .filter { (_, types) -> types.size in 5..7 && types[4] == kind }
                .maxByOrNull { (_, types) -> types.size }
            candidates(RECT)?.let { (index, types) -> return Frame(index, types.size) }
            candidates(INSETS)?.let { (index, types) -> return Size(index, types.size) }
            return null
        }
    }
}
