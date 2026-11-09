package io.github.thisisthepy.dsa.agent.platform

import android.app.ActivityManager
import android.content.Context
import android.content.pm.PackageManager
import android.provider.DeviceConfig
import android.provider.Settings
import io.github.thisisthepy.dsa.agent.memory.AppProcesses
import java.io.File

/**
 * The guest's app processes through the activity manager. Ending an app is
 * `ActivityManager.forceStopPackage`, which is not in the SDK and is called by reflection; the
 * agent is a platform app and holds FORCE_STOP_PACKAGES. Its running services are visible
 * because the agent is a system app, which `getRunningServices` answers for every app.
 */
class AndroidAppProcesses(context: Context) : AppProcesses {
    private val activities = context.getSystemService(ActivityManager::class.java)
    private val packages = context.packageManager
    private val resolver = context.contentResolver

    private val mainExecutor = context.mainExecutor
    private var watching = false

    private companion object {
        const val AM_CONSTANTS = "activity_manager_constants"
        const val NAMESPACE = "activity_manager"
        const val MAX_CACHED = "max_cached_processes"
    }

    @Suppress("DEPRECATION")
    override fun hasForegroundService(packageName: String): Boolean =
        activities.getRunningServices(Int.MAX_VALUE).orEmpty()
            .any { it.foreground && it.service.packageName == packageName }

    override fun stop(packageName: String) {
        ActivityManager::class.java
            .getMethod("forceStopPackage", String::class.java)
            .invoke(activities, packageName)
    }

    override fun disable(packageName: String): Boolean {
        val state = try {
            packages.getApplicationEnabledSetting(packageName)
        } catch (e: IllegalArgumentException) {
            return false // not installed
        }
        if (state == PackageManager.COMPONENT_ENABLED_STATE_DISABLED) return false
        packages.setApplicationEnabledSetting(packageName, PackageManager.COMPONENT_ENABLED_STATE_DISABLED, 0)
        return true
    }

    /**
     * The activity manager reads the cached process limit from the device configuration
     * (namespace activity_manager, key max_cached_processes), and only without one from the
     * framework's config_customizedMaxCachedProcesses, which the image's framework overlay sets
     * to the same limit. The global setting activity_manager_constants no longer carries it:
     * Android 17 parses that setting for its other limits and ignores this key, so the limit
     * written there before never applied and the activity manager kept up to 1024.
     *
     * The flag is set now, takes effect at once, and is set again whenever something else
     * changes it (Google Play services' flag sync may), so the limit holds for as long as the
     * agent runs. What earlier agents wrote to the global setting is taken back.
     */
    override fun limitCachedProcesses(max: Int) {
        val wanted = max.toString()
        if (DeviceConfig.getProperty(NAMESPACE, MAX_CACHED) != wanted) {
            DeviceConfig.setProperty(NAMESPACE, MAX_CACHED, wanted, false)
        }
        if (!watching) {
            watching = true
            DeviceConfig.addOnPropertiesChangedListener(NAMESPACE, mainExecutor) { changed ->
                if (MAX_CACHED in changed.keyset && changed.getString(MAX_CACHED, null) != wanted) {
                    DeviceConfig.setProperty(NAMESPACE, MAX_CACHED, wanted, false)
                }
            }
        }
        val legacy = Settings.Global.getString(resolver, AM_CONSTANTS)
        if (legacy != null && legacy.matches(Regex("$MAX_CACHED=\\d+"))) {
            Settings.Global.putString(resolver, AM_CONSTANTS, null)
        }
    }

    override fun freeKb(): Long =
        File("/proc/meminfo").useLines { lines ->
            lines.firstOrNull { it.startsWith("MemFree:") }
                ?.substringAfter(':')?.trim()?.substringBefore(' ')?.toLongOrNull() ?: 0L
        }
}
