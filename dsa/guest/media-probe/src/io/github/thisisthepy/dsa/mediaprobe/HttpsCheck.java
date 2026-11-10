// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

package io.github.thisisthepy.dsa.mediaprobe;

import android.os.Bundle;
import android.util.Log;

import java.io.IOException;
import java.io.InputStream;
import java.net.URL;

import javax.net.ssl.HttpsURLConnection;
import javax.net.ssl.SSLException;

/**
 * Opens an HTTPS address as an ordinary app does, with the platform's default trust: the
 * system certificates under the app's network security config. It tells apart a connection
 * that worked, one the certificate check refused, and one that never reached a server:
 *
 * <pre>
 *   am instrument -w -r -e check https -e url https://localhost:8443/ \
 *       io.github.thisisthepy.dsa.mediaprobe/.MediaProbe
 * </pre>
 *
 * The results are "outcome" ("connected", "untrusted" or "unreachable"), the HTTP "status"
 * when connected, and the exception's "detail" otherwise.
 */
final class HttpsCheck {
    private static final int TIMEOUT_MS = 15_000;

    private HttpsCheck() {}

    static void run(String url, Bundle results) {
        HttpsURLConnection connection = null;
        try {
            connection = (HttpsURLConnection) new URL(url).openConnection();
            connection.setConnectTimeout(TIMEOUT_MS);
            connection.setReadTimeout(TIMEOUT_MS);
            connection.setUseCaches(false);
            int status = connection.getResponseCode();
            try (InputStream body = status < 400 ? connection.getInputStream() : connection.getErrorStream()) {
                if (body != null) {
                    byte[] buffer = new byte[4096];
                    while (body.read(buffer) >= 0) {
                        // Read to the end, so the whole exchange happened.
                    }
                }
            }
            results.putString("outcome", "connected");
            results.putString("status", Integer.toString(status));
        } catch (SSLException e) {
            Log.i(MediaProbe.TAG, "the certificate check refused " + url, e);
            results.putString("outcome", "untrusted");
            results.putString("detail", String.valueOf(e));
        } catch (IOException e) {
            Log.i(MediaProbe.TAG, url + " cannot be reached", e);
            results.putString("outcome", "unreachable");
            results.putString("detail", String.valueOf(e));
        } finally {
            if (connection != null) {
                connection.disconnect();
            }
        }
    }
}
