package android.location.provider;

import android.content.Context;
import android.location.Location;
import android.os.Bundle;
import android.os.IBinder;

/**
 * The framework's base class for a location provider in another process, which the location
 * manager binds (a system API). Declarations only: the class comes from the framework.
 */
public abstract class LocationProviderBase {
    /** The action a network location provider's service answers. */
    public static final String ACTION_NETWORK_PROVIDER =
            "com.android.location.service.v3.NetworkLocationProvider";

    /** Called when a flush the location manager asked for is done. */
    public interface OnFlushCompleteCallback {
        void onFlushComplete();
    }

    public LocationProviderBase(Context context, String tag, ProviderProperties properties) {
        throw new RuntimeException("Stub!");
    }

    public final IBinder getBinder() {
        throw new RuntimeException("Stub!");
    }

    public void setAllowed(boolean allowed) {
        throw new RuntimeException("Stub!");
    }

    public boolean isAllowed() {
        throw new RuntimeException("Stub!");
    }

    public void reportLocation(Location location) {
        throw new RuntimeException("Stub!");
    }

    public abstract void onSetRequest(ProviderRequest request);

    public abstract void onFlush(OnFlushCompleteCallback callback);

    public abstract void onSendExtraCommand(String command, Bundle extras);
}
