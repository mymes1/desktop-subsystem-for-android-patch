package io.github.thisisthepy.dsa.agent.platform

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import io.github.thisisthepy.dsa.agent.gapps.GoogleAppsCompile
import io.github.thisisthepy.dsa.agent.gapps.GoogleAppsCompilePort

/**
 * The package manager's and the power manager's broadcasts, and the request property, which
 * init lets the agent set (sepolicy/gapps.cil). SystemProperties is not in the SDK and is
 * called by reflection.
 */
class AndroidGoogleAppsCompile(private val context: Context) : GoogleAppsCompilePort {
    override fun listen(onPackage: (String) -> Unit, onScreenOff: () -> Unit) {
        val packages = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                intent.data?.schemeSpecificPart?.let(onPackage)
            }
        }
        val screen = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) = onScreenOff()
        }
        // An update sends ACTION_PACKAGE_ADDED (with EXTRA_REPLACING) and ACTION_PACKAGE_REPLACED;
        // asking twice costs one more status read.
        val changes = IntentFilter().apply {
            addAction(Intent.ACTION_PACKAGE_ADDED)
            addAction(Intent.ACTION_PACKAGE_REPLACED)
            addDataScheme("package")
        }
        // Only the system sends these.
        context.registerReceiver(packages, changes, Context.RECEIVER_NOT_EXPORTED)
        context.registerReceiver(screen, IntentFilter(Intent.ACTION_SCREEN_OFF), Context.RECEIVER_NOT_EXPORTED)
    }

    override fun request(value: String) {
        Class.forName("android.os.SystemProperties")
            .getMethod("set", String::class.java, String::class.java)
            .invoke(null, GoogleAppsCompile.PROPERTY, value)
    }
}
