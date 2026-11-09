package io.github.thisisthepy.dsa.agent.platform

import android.app.ActivityManager
import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import io.github.thisisthepy.dsa.agent.memory.GooglePlayPackages
import io.github.thisisthepy.dsa.agent.memory.MemorySource
import io.github.thisisthepy.dsa.agent.memory.ProcessPss
import java.io.File

/**
 * The guest's memory as the kernel and the activity manager see it.
 *
 * The activity manager answers for every process it runs because the agent holds
 * REAL_GET_TASKS; without it, it would answer for the agent's own process alone. Native
 * daemons are not among them, and the kernel's totals cover them.
 */
class AndroidMemorySource(context: Context) : MemorySource {
    private val activities = context.getSystemService(ActivityManager::class.java)
    private val packages = context.packageManager

    // Whether each package is one of the Google apps. The answer does not change while Android
    // runs: an update keeps a system app a system app.
    private val googlePlay = HashMap<String, Boolean>()

    private fun isGooglePlay(packageName: String): Boolean = googlePlay.getOrPut(packageName) {
        val info = try {
            packages.getApplicationInfo(packageName, 0)
        } catch (e: PackageManager.NameNotFoundException) {
            null
        }
        val system = info != null && info.flags and ApplicationInfo.FLAG_SYSTEM != 0
        // init mounts the Google apps image at GAPPS_IMAGE and lays it over the system's
        // folders, so an app it brought is at the same path under the image. An update moves
        // the APK to /data, and then the name decides.
        val fromImage = info?.sourceDir?.let { path ->
            runCatching { File(GAPPS_IMAGE + path).exists() }.getOrDefault(false)
        } ?: false
        GooglePlayPackages.isGooglePlay(packageName, system, fromImage)
    }

    override fun meminfo(): String = File("/proc/meminfo").readText()

    override fun zramMmStat(): String? {
        val file = File("/sys/devices/virtual/block/zram0/mm_stat")
        return try {
            file.readText()
        } catch (e: Exception) {
            // No zram, or the policy keeps the agent from reading it: the report goes without.
            null
        }
    }

    /** The online memory blocks times the block size, from sysfs; null when policy hides them. */
    override fun memOnlineKb(): Long? {
        val memory = File("/sys/devices/system/memory")
        val blockBytes = File(memory, "block_size_bytes").readText().trim().toLong(16)
        val online = memory.listFiles { file -> file.name.startsWith("memory") }.orEmpty()
            .count { block -> runCatching { File(block, "online").readText().trim() == "1" }.getOrDefault(false) }
        return if (online == 0) null else online * blockBytes / 1024
    }

    override fun processes(): List<ProcessPss> {
        val running = activities.runningAppProcesses ?: return emptyList()
        if (running.isEmpty()) return emptyList()
        val info = activities.getProcessMemoryInfo(running.map { it.pid }.toIntArray())
        return running.indices.mapNotNull { i ->
            val memory = info.getOrNull(i) ?: return@mapNotNull null
            ProcessPss(
                pid = running[i].pid,
                name = running[i].processName,
                pssKb = memory.totalPss.toLong(),
                // "summary.total-swap" is the swapped-out PSS in KiB, as dumpsys meminfo shows it.
                swapPssKb = memory.getMemoryStat("summary.total-swap")?.toLongOrNull() ?: 0,
                // A process runs the code of each package in its list; the Google apps share
                // their processes (gms, gsf), so any one of them makes it Google Play's.
                googlePlay = running[i].pkgList.orEmpty().any(::isGooglePlay),
            )
        }
    }

    private companion object {
        /** Where init mounts the Google apps image before laying it over the system. */
        const val GAPPS_IMAGE = "/mnt/dsa-gapps"
    }
}
