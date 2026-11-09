package android.location.provider;

import android.content.Context;
import android.location.Address;
import android.os.IBinder;
import android.os.OutcomeReceiver;
import java.util.List;

/**
 * The framework's base class for the geocode provider in another process, which the location
 * manager binds for Android's Geocoder (a system API). Declarations only: the class comes from
 * the framework.
 */
public abstract class GeocodeProviderBase {
    /** The action the geocode provider's service answers. */
    public static final String ACTION_GEOCODE_PROVIDER = "com.android.location.service.GeocodeProvider";

    public GeocodeProviderBase(Context context, String tag) {
        throw new RuntimeException("Stub!");
    }

    public final IBinder getBinder() {
        throw new RuntimeException("Stub!");
    }

    public abstract void onForwardGeocode(
            ForwardGeocodeRequest request, OutcomeReceiver<List<Address>, Throwable> callback);

    public abstract void onReverseGeocode(
            ReverseGeocodeRequest request, OutcomeReceiver<List<Address>, Throwable> callback);
}
