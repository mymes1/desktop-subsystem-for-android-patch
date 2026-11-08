package android.view;

import android.graphics.Region;
import android.os.Binder;
import android.os.IBinder;
import android.os.IInterface;
import android.os.RemoteException;

/**
 * The window manager's callback with the parts of a display where apps keep system gestures
 * out (a oneway AIDL interface); declarations only.
 */
public interface ISystemGestureExclusionListener extends IInterface {
    void onSystemGestureExclusionChanged(int displayId, Region systemGestureExclusion,
            Region systemGestureExclusionUnrestricted) throws RemoteException;

    abstract class Stub extends Binder implements ISystemGestureExclusionListener {
        public Stub() {
            throw new RuntimeException("Stub!");
        }

        @Override
        public IBinder asBinder() {
            throw new RuntimeException("Stub!");
        }
    }
}
