package android.location.provider;

/**
 * What the location manager wants of a location provider (a system API). Declarations only:
 * the class comes from the framework.
 */
public final class ProviderRequest {
    /** The interval of a request that wants no location. */
    public static final long INTERVAL_DISABLED = Long.MAX_VALUE;

    private ProviderRequest() {
        throw new RuntimeException("Stub!");
    }

    public boolean isActive() {
        throw new RuntimeException("Stub!");
    }

    public long getIntervalMillis() {
        throw new RuntimeException("Stub!");
    }
}
