// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0
package io.github.thisisthepy.dsa.mediaprobe;

import android.app.Activity;
import android.os.Bundle;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.widget.LinearLayout;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/**
 * Two surfaces on screen, one above the other, for the probe's video checks: a decoder shows
 * its pictures on the first and moves to the second partway, as a player does when its view
 * changes, or between clips, as a feed of short videos does. Each is a SurfaceView, so its
 * buffers go to the composer as an app's video does.
 */
public final class VideoActivity extends Activity {
    private final SurfaceView[] views = new SurfaceView[2];
    private final CountDownLatch ready = new CountDownLatch(2);

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        for (int i = 0; i < views.length; i++) {
            SurfaceView view = new SurfaceView(this);
            view.getHolder().addCallback(new SurfaceHolder.Callback() {
                @Override
                public void surfaceCreated(SurfaceHolder holder) {
                    ready.countDown();
                }

                @Override
                public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {}

                @Override
                public void surfaceDestroyed(SurfaceHolder holder) {}
            });
            layout.addView(view, new LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f));
            views[i] = view;
        }
        setContentView(layout);
    }

    /** Waits up to `ms` for both surfaces to exist. */
    boolean awaitSurfaces(long ms) throws InterruptedException {
        return ready.await(ms, TimeUnit.MILLISECONDS);
    }

    SurfaceView view(int index) {
        return views[index];
    }

    Surface surface(int index) {
        return views[index].getHolder().getSurface();
    }
}
