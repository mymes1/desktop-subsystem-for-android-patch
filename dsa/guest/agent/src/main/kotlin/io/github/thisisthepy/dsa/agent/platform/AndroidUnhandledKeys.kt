package io.github.thisisthepy.dsa.agent.platform

import android.content.Context
import android.hardware.input.InputManager
import android.view.KeyCharacterMap
import android.view.KeyEvent
import io.github.thisisthepy.dsa.agent.Log
import io.github.thisisthepy.dsa.agent.input.UnhandledKeys
import java.lang.reflect.Proxy
import java.util.concurrent.Executor

/**
 * Hears the Escapes no app took, from the framework's key gesture events
 * (InputManager.registerKeyGestureEventListener, which needs MANAGE_KEY_GESTURES, a signature
 * permission the agent holds as a platform app).
 *
 * When the focused window does not handle a plain Escape, the input manager's unhandled key
 * stage turns it into KEY_GESTURE_TYPE_CLOSE_ALL_DIALOGS; no other path makes that gesture. So
 * the gesture, from the virtual keyboard the agent's keys come from, with Escape as its key,
 * is the word that the app left the agent's Escape. The listener interface and the event are
 * not in the SDK and are reached by reflection.
 */
class AndroidUnhandledKeys(private val context: Context, private val log: Log) : UnhandledKeys {
    private var listener: Any? = null

    override fun listen(onEscape: () -> Unit): Boolean = try {
        val manager = context.getSystemService(InputManager::class.java)
        val listenerClass = Class.forName("android.hardware.input.InputManager\$KeyGestureEventListener")
        val eventClass = Class.forName("android.hardware.input.KeyGestureEvent")
        val gestureType = eventClass.getMethod("getKeyGestureType")
        val deviceId = eventClass.getMethod("getDeviceId")
        val keycodes = eventClass.getMethod("getKeycodes")
        val callback = Proxy.newProxyInstance(listenerClass.classLoader, arrayOf(listenerClass)) { proxy, method, args ->
            when (method.name) {
                "onKeyGestureEvent" -> {
                    val event = args?.getOrNull(0)
                    if (event != null &&
                        gestureType.invoke(event) == KEY_GESTURE_TYPE_CLOSE_ALL_DIALOGS &&
                        deviceId.invoke(event) == KeyCharacterMap.VIRTUAL_KEYBOARD &&
                        (keycodes.invoke(event) as? IntArray)?.contains(KeyEvent.KEYCODE_ESCAPE) == true
                    ) {
                        onEscape()
                    }
                    null
                }
                "hashCode" -> System.identityHashCode(proxy)
                "equals" -> proxy === args?.getOrNull(0)
                "toString" -> "the agent's unhandled Escape listener"
                else -> null
            }
        }
        InputManager::class.java
            .getMethod("registerKeyGestureEventListener", Executor::class.java, listenerClass)
            .invoke(manager, Executor { it.run() }, callback)
        listener = callback
        true
    } catch (e: ReflectiveOperationException) {
        log.warn("could not hear the Escapes no app takes; Esc never goes back", e)
        false
    } catch (e: LinkageError) {
        log.warn("this framework does not say which Escapes no app takes; Esc never goes back", e)
        false
    } catch (e: SecurityException) {
        log.warn("the framework refused to say which Escapes no app takes; Esc never goes back", e)
        false
    }

    private companion object {
        // KeyGestureEvent.KEY_GESTURE_TYPE_CLOSE_ALL_DIALOGS (Android 16 and 17).
        const val KEY_GESTURE_TYPE_CLOSE_ALL_DIALOGS = 61
    }
}
