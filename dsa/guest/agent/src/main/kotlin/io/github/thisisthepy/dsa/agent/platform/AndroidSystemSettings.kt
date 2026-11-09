package io.github.thisisthepy.dsa.agent.platform

import android.app.AlarmManager
import android.app.UiModeManager
import android.content.Context
import android.content.om.FabricatedOverlay
import android.content.om.OverlayManager
import android.content.om.OverlayManagerTransaction
import android.os.LocaleList
import android.os.Process
import android.provider.Settings
import android.util.TypedValue
import io.github.thisisthepy.dsa.agent.settings.SystemSettingsPort
import org.json.JSONObject

/**
 * The guest's system settings, as the agent, a platform app, changes them.
 *
 * The time zone goes through the alarm manager, as the Settings app sets it. The system
 * languages go through the framework's locale picker, which updates the persistent
 * configuration, so every app is told and they last across restarts. The 24-hour clock is the
 * system setting Settings writes. Dark mode is the UI mode manager's night mode.
 *
 * The accent color has no single switch: on a phone, SystemUI turns the theme setting into an
 * overlay of the framework's dynamic colors. The guest has no SystemUI, so the agent writes the
 * same setting (for anything that reads it) and registers that overlay itself: a fabricated
 * overlay of the `android` package with the palette's colors, enabled for every user.
 */
class AndroidSystemSettings(private val context: Context) : SystemSettingsPort {
    override fun setTimeZone(id: String) {
        context.getSystemService(AlarmManager::class.java).setTimeZone(id)
    }

    override fun setLocales(tags: List<String>) {
        val locales = LocaleList.forLanguageTags(tags.joinToString(","))
        // com.android.internal.app.LocalePicker: not in the SDK; platform apps may call it.
        Class.forName("com.android.internal.app.LocalePicker")
            .getMethod("updateLocales", LocaleList::class.java)
            .invoke(null, locales)
    }

    override fun setTime24Hour(on: Boolean) {
        Settings.System.putString(context.contentResolver, Settings.System.TIME_12_24, if (on) "24" else "12")
    }

    override fun fontScale(): Float =
        Settings.System.getFloat(context.contentResolver, Settings.System.FONT_SCALE, 1f)

    /**
     * The font scale Settings' font size writes. The activity task manager hears the setting
     * change and gives every app the new scale in its configuration.
     */
    override fun setFontScale(scale: Float) {
        Settings.System.putFloat(context.contentResolver, Settings.System.FONT_SCALE, scale)
    }

    override fun setDarkMode(on: Boolean) {
        context.getSystemService(UiModeManager::class.java)
            .setNightMode(if (on) UiModeManager.MODE_NIGHT_YES else UiModeManager.MODE_NIGHT_NO)
    }

    override fun setAccentColor(argb: Int, palette: Map<String, Int>) {
        val seed = "%06X".format(argb and 0xffffff)
        val theme = JSONObject()
            .put("android.theme.customization.system_palette", seed)
            .put("android.theme.customization.accent_color", seed)
            .put("android.theme.customization.color_source", "preset")
            .put("android.theme.customization.theme_style", "TONAL_SPOT")
            .put("_applied_timestamp", System.currentTimeMillis())
        Settings.Secure.putString(context.contentResolver, THEME_SETTING, theme.toString())

        val overlay = FabricatedOverlay(OVERLAY_NAME, "android")
        for ((name, color) in palette) {
            overlay.setResourceValue("android:color/$name", TypedValue.TYPE_INT_COLOR_ARGB8, color, null)
        }
        val overlays = context.getSystemService(OverlayManager::class.java)
        val register = OverlayManagerTransaction.newInstance()
        register.registerFabricatedOverlay(overlay)
        overlays.commit(register)
        // Enabling an overlay of another package is not in the SDK; platform apps may call
        // OverlayManagerTransaction.Builder for it. A UserHandle's hash is its user ID.
        val builderClass = Class.forName("android.content.om.OverlayManagerTransaction\$Builder")
        val builder = builderClass.getConstructor().newInstance()
        builderClass
            .getMethod(
                "setEnabled",
                android.content.om.OverlayIdentifier::class.java,
                Boolean::class.javaPrimitiveType,
                Int::class.javaPrimitiveType,
            )
            .invoke(builder, overlay.identifier, true, Process.myUserHandle().hashCode())
        val enable = builderClass.getMethod("build").invoke(builder) as OverlayManagerTransaction
        overlays.commit(enable)
    }

    private companion object {
        /** Settings.Secure.THEME_CUSTOMIZATION_OVERLAY_PACKAGES, which SystemUI reads. */
        const val THEME_SETTING = "theme_customization_overlay_packages"

        /** The fabricated overlay that carries the palette. */
        const val OVERLAY_NAME = "dsa_dynamic_colors"
    }
}
