package io.github.thisisthepy.dsa.agent.platform

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.content.pm.VersionedPackage
import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.agent.install.AppKind
import io.github.thisisthepy.dsa.agent.install.InstallerResult
import io.github.thisisthepy.dsa.agent.install.PackageUninstallerPort
import java.util.concurrent.atomic.AtomicInteger

/**
 * Uninstalls apps through the framework's PackageInstaller.
 *
 * The agent is a privileged platform app holding DELETE_PACKAGES, so an uninstall goes ahead
 * without asking the user again in the guest: the host has asked already. Should the installer
 * still ask (STATUS_PENDING_USER_ACTION), the agent shows its confirmation, which opens as a
 * task window on the host, and waits for the final answer. Each uninstall gets a broadcast
 * action of its own, delivered only to this process, which carries the installer's answer.
 */
class AndroidPackageUninstaller(private val context: Context, private val log: Log) : PackageUninstallerPort {
    override fun kind(packageName: String): AppKind? {
        val info = try {
            context.packageManager.getApplicationInfo(packageName, PackageManager.ApplicationInfoFlags.of(0))
        } catch (e: PackageManager.NameNotFoundException) {
            return null
        }
        val system = info.flags and (ApplicationInfo.FLAG_SYSTEM or ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) != 0
        return if (system) AppKind.SYSTEM else AppKind.USER
    }

    override fun uninstall(packageName: String, onResult: (InstallerResult) -> Unit) {
        val action = "${context.packageName}.UNINSTALL_RESULT.${NEXT.incrementAndGet()}"
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
                if (status == PackageInstaller.STATUS_PENDING_USER_ACTION) {
                    val confirmation = intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                    if (confirmation == null) {
                        log.warn("Android asked to confirm uninstalling $packageName but gave nothing to show")
                    } else {
                        log.info("Android asks the user to confirm uninstalling $packageName")
                        context.startActivity(confirmation.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                    }
                    return
                }
                context.unregisterReceiver(this)
                onResult(
                    InstallerResult(
                        status = status,
                        message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE),
                        packageName = packageName,
                    ),
                )
            }
        }
        context.registerReceiver(receiver, IntentFilter(action), Context.RECEIVER_NOT_EXPORTED)
        val callback = PendingIntent.getBroadcast(
            context,
            0,
            Intent(action).setPackage(context.packageName),
            // The installer fills the status in, so the intent must be mutable.
            PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        try {
            context.packageManager.packageInstaller.uninstall(
                VersionedPackage(packageName, PackageManager.VERSION_CODE_HIGHEST),
                callback.intentSender,
            )
        } catch (e: Exception) {
            context.unregisterReceiver(receiver)
            throw e
        }
    }

    private companion object {
        val NEXT = AtomicInteger()
    }
}
