package io.github.thisisthepy.dsa.agent.install

import io.github.thisisthepy.dsa.agent.Log

/** What the agent hears and sets for the compile of the Google apps without an app image. */
interface GoogleAppsCompilePort {
    /**
     * Calls [onPackage] with the name of each package installed or updated, and [onScreenOff]
     * each time the screen goes off.
     */
    fun listen(onPackage: (String) -> Unit, onScreenOff: () -> Unit)

    /** Sets the request property to [value]; init runs the compile when it changes. */
    fun request(value: String)
}

/**
 * Asks init to compile Google Play services and the Play Store again without an app image
 * (the image's dsa-gapps.rc and gapps-compile.sh). ART gives every app the Play Store installs
 * or updates an app image, and its background job gives one again when it compiles an app while
 * the device is idle; each process of the app then loads its own copy, 25-30 MiB for each of
 * the two Play services processes seen running on a PC. So the agent asks when either package
 * is installed or updated, and each time the screen goes off, when no window is shown and the
 * compile takes nothing from the user's apps. The script compiles only a package that has an
 * app image, so asking costs little otherwise. Without Google Play init ignores the request.
 *
 * Each request is a value the property has not had, the [clock]'s reading, because init acts
 * only when the property changes.
 */
class GoogleAppsCompile(
    private val port: GoogleAppsCompilePort,
    private val clock: () -> Long,
    private val log: Log,
) {
    private var last = Long.MIN_VALUE

    fun start() {
        port.listen(onPackage = ::packageChanged, onScreenOff = { ask("the screen went off") })
    }

    private fun packageChanged(packageName: String) {
        if (packageName in PACKAGES) ask("$packageName was installed or updated")
    }

    private fun ask(why: String) {
        val now = clock().let { if (it <= last) last + 1 else it }
        last = now
        try {
            port.request(now.toString())
            log.info("asked for the Google apps to be compiled without an app image: $why")
        } catch (e: Exception) {
            log.warn("could not ask for the Google apps to be compiled without an app image ($why)", e)
        }
    }

    companion object {
        /** The packages the script compiles, as gapps-compile.sh names them. */
        val PACKAGES = setOf("com.google.android.gms", "com.android.vending")

        /** The property init watches (vendor_property_contexts, sepolicy/gapps.cil). */
        const val PROPERTY = "vendor.dsa.gapps.compile"
    }
}
