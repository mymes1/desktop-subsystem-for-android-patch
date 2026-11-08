// Copyright 2026 The DSA Authors
// SPDX-License-Identifier: Apache-2.0

package io.github.thisisthepy.dsa.mediaprobe;

import android.app.Activity;
import android.app.Instrumentation;
import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioTrack;
import android.os.Bundle;
import android.os.SystemClock;
import android.util.Log;

/**
 * Plays sound and runs codecs in an ordinary app's process, and reports what happened as the
 * instrumentation's results, one key per line of `am instrument -r`:
 *
 * <pre>
 *   am instrument -w -r -e check audio io.github.thisisthepy.dsa.mediaprobe/.MediaProbe
 * </pre>
 *
 * The "audio" check plays a second of a 440 Hz tone through a streaming track and a static
 * one. A streaming track's control block, and a static track's shared buffer, are memory an
 * app shares with the audio server, which is what an app whose process the policy keeps apart
 * cannot do when the rules are missing.
 */
public final class MediaProbe extends Instrumentation {
    private static final String TAG = "DsaMediaProbe";
    private static final int RATE = 48_000;
    private static final int CHANNELS = 2;
    private static final int FRAME_BYTES = 2 * CHANNELS;

    private String check = "audio";

    @Override
    public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        if (arguments != null && arguments.getString("check") != null) {
            check = arguments.getString("check");
        }
        start();
    }

    @Override
    public void onStart() {
        Bundle results = new Bundle();
        try {
            if (check.equals("audio")) {
                audio(results);
            } else {
                results.putString("error", "no check named " + check);
            }
        } catch (Throwable t) {
            Log.e(TAG, "the " + check + " check failed", t);
            results.putString("error", t.toString());
        }
        finish(Activity.RESULT_OK, results);
    }

    /** One second of a 440 Hz tone, 16-bit stereo at 48 kHz. */
    private static short[] tone() {
        short[] pcm = new short[RATE * CHANNELS];
        for (int i = 0; i < RATE; i++) {
            short sample = (short) (Math.sin(2 * Math.PI * 440 * i / RATE) * 8000);
            pcm[CHANNELS * i] = sample;
            pcm[CHANNELS * i + 1] = sample;
        }
        return pcm;
    }

    private static AudioTrack track(int mode, int bytes) {
        return new AudioTrack.Builder()
                .setAudioAttributes(new AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build())
                .setAudioFormat(new AudioFormat.Builder()
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .setSampleRate(RATE)
                        .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                        .build())
                .setTransferMode(mode)
                .setBufferSizeInBytes(bytes)
                .build();
    }

    /** Waits up to `wait` ms for the track to have played at least `frames` frames. */
    private static int playedFrames(AudioTrack track, int frames, long wait) {
        long deadline = SystemClock.uptimeMillis() + wait;
        int played = track.getPlaybackHeadPosition();
        while (played < frames && SystemClock.uptimeMillis() < deadline) {
            SystemClock.sleep(50);
            played = track.getPlaybackHeadPosition();
        }
        return played;
    }

    private void audio(Bundle results) {
        short[] pcm = tone();

        // A streaming track: the app writes into the control block's ring as it plays.
        AudioTrack stream = null;
        try {
            int min = AudioTrack.getMinBufferSize(RATE, AudioFormat.CHANNEL_OUT_STEREO,
                    AudioFormat.ENCODING_PCM_16BIT);
            stream = track(AudioTrack.MODE_STREAM, Math.max(min, RATE / 10 * FRAME_BYTES));
            results.putInt("audio.stream.state", stream.getState());
            stream.play();
            int written = stream.write(pcm, 0, pcm.length, AudioTrack.WRITE_BLOCKING);
            results.putInt("audio.stream.written", written < 0 ? written : written / CHANNELS);
            results.putInt("audio.stream.played", playedFrames(stream, RATE / 2, 3000));
            stream.stop();
        } catch (RuntimeException e) {
            Log.e(TAG, "the streaming track failed", e);
            results.putString("audio.stream.error", e.toString());
        } finally {
            if (stream != null) {
                stream.release();
            }
        }

        // A static track: the app's whole sound in a buffer it shares with the audio server.
        AudioTrack fixed = null;
        try {
            fixed = track(AudioTrack.MODE_STATIC, pcm.length * 2);
            int written = fixed.write(pcm, 0, pcm.length);
            results.putInt("audio.static.written", written < 0 ? written : written / CHANNELS);
            results.putInt("audio.static.state", fixed.getState());
            fixed.play();
            results.putInt("audio.static.played", playedFrames(fixed, RATE / 2, 3000));
            fixed.stop();
        } catch (RuntimeException e) {
            Log.e(TAG, "the static track failed", e);
            results.putString("audio.static.error", e.toString());
        } finally {
            if (fixed != null) {
                fixed.release();
            }
        }
    }
}
