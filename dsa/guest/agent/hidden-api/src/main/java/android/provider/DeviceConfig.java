package android.provider;

import java.util.Set;
import java.util.concurrent.Executor;

/**
 * The framework's server-configurable flags, by namespace; declarations only. The activity
 * manager reads its cached process limit from here (namespace "activity_manager", key
 * "max_cached_processes"), not from the global setting it once read it from.
 */
public final class DeviceConfig {
    private DeviceConfig() {
        throw new RuntimeException("Stub!");
    }

    public static String getProperty(String namespace, String name) {
        throw new RuntimeException("Stub!");
    }

    public static boolean setProperty(String namespace, String name, String value, boolean makeDefault) {
        throw new RuntimeException("Stub!");
    }

    public static void addOnPropertiesChangedListener(
            String namespace, Executor executor, OnPropertiesChangedListener listener) {
        throw new RuntimeException("Stub!");
    }

    /** Told which flags of a namespace changed. */
    public interface OnPropertiesChangedListener {
        void onPropertiesChanged(Properties properties);
    }

    /** The flags of one namespace that changed, and their values. */
    public static class Properties {
        private Properties() {
            throw new RuntimeException("Stub!");
        }

        public String getNamespace() {
            throw new RuntimeException("Stub!");
        }

        public Set<String> getKeyset() {
            throw new RuntimeException("Stub!");
        }

        public String getString(String name, String defaultValue) {
            throw new RuntimeException("Stub!");
        }
    }
}
