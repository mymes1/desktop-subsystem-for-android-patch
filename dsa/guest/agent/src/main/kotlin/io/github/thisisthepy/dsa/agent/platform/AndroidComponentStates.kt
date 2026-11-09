package io.github.thisisthepy.dsa.agent.platform

import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import io.github.thisisthepy.dsa.agent.gapps.ComponentStates

/**
 * Other apps' components through the package manager. Changing them needs
 * CHANGE_COMPONENT_ENABLED_STATE, which the agent holds as a privileged platform app.
 */
class AndroidComponentStates(private val context: Context) : ComponentStates {
    private val packages = context.packageManager

    override fun installed(packageName: String): Boolean = try {
        packages.getApplicationInfo(packageName, PackageManager.ApplicationInfoFlags.of(0))
        true
    } catch (e: PackageManager.NameNotFoundException) {
        false
    }

    /**
     * Null when the package declares no such service, receiver or provider: the package
     * manager refuses a whole batch that names a component that does not exist.
     */
    override fun isOff(packageName: String, className: String): Boolean? {
        val component = ComponentName(packageName, className)
        if (!declared(component)) return null
        return packages.getComponentEnabledSetting(component) == PackageManager.COMPONENT_ENABLED_STATE_DISABLED
    }

    private fun declared(component: ComponentName): Boolean {
        val flags = PackageManager.ComponentInfoFlags.of(
            (PackageManager.MATCH_DISABLED_COMPONENTS or PackageManager.MATCH_DIRECT_BOOT_AWARE or
                PackageManager.MATCH_DIRECT_BOOT_UNAWARE).toLong(),
        )
        val lookups: List<() -> Unit> = listOf(
            { packages.getServiceInfo(component, flags) },
            { packages.getReceiverInfo(component, flags) },
            { packages.getProviderInfo(component, flags) },
        )
        return lookups.any { lookup ->
            try {
                lookup()
                true
            } catch (e: PackageManager.NameNotFoundException) {
                false
            }
        }
    }

    /**
     * DONT_KILL_APP keeps the package's process: Play services' push connection and sign-ins
     * live in the same process as the services turned off, and the activity manager stops a
     * disabled service without ending its process.
     */
    override fun turnOff(packageName: String, classNames: List<String>) {
        packages.setComponentEnabledSettings(
            classNames.map { className ->
                PackageManager.ComponentEnabledSetting(
                    ComponentName(packageName, className),
                    PackageManager.COMPONENT_ENABLED_STATE_DISABLED,
                    PackageManager.DONT_KILL_APP,
                )
            },
        )
    }

    override fun listen(onChange: (String) -> Unit) {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                intent.data?.schemeSpecificPart?.let(onChange)
            }
        }
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_PACKAGE_ADDED)
            addAction(Intent.ACTION_PACKAGE_REPLACED)
            addAction(Intent.ACTION_PACKAGE_CHANGED)
            addDataScheme("package")
        }
        // Only the system sends these, so no other app can make the agent change anything.
        context.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
    }
}
