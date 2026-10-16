package dev.gpui.mobile;

import android.app.Activity;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import java.util.concurrent.Callable;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import android.support.v4.media.MediaMetadataCompat;
import android.support.v4.media.session.MediaSessionCompat;
import android.support.v4.media.session.PlaybackStateCompat;
import android.util.Log;

import androidx.core.app.NotificationCompat;
import androidx.media.session.MediaButtonReceiver;

/**
 * Manages an Android MediaSession for audio/video playback.
 *
 * Provides:
 * - System media notification with playback controls
 * - Lock screen and system panel playback info
 * - Media button handling (headphone buttons, car controls)
 * - Volume key integration
 *
 * All methods are static and called from Rust via JNI.
 */
public class GpuiMediaSession {

    private static final String TAG = "GpuiMediaSession";
    private static final String CHANNEL_ID = "gpui_media_playback";
    private static final int NOTIFICATION_ID = 1001;

    private static MediaSessionCompat sSession;
    private static NotificationManager sNotificationManager;
    private static Activity sActivity;
    private static final Handler mainHandler = new Handler(Looper.getMainLooper());

    private static final class MainOperation<T> implements Runnable {
        final Callable<T> operation;
        final CountDownLatch done = new CountDownLatch(1);
        boolean abandoned;
        boolean finished;
        T value;
        Throwable failure;
        MainOperation(Callable<T> operation) { this.operation = operation; }
        @Override public void run() {
            synchronized (this) {
                if (abandoned) return; // Cancellation wins before start.
            }
            T result = null;
            Throwable error = null;
            try { result = operation.call(); }
            catch (Throwable caught) { error = caught; }
            boolean cleanup;
            synchronized (this) {
                // Completion and abandonment have one linearization point.
                // A late init/mutation must not leave a live orphan session.
                cleanup = abandoned;
                value = result;
                failure = error;
                finished = true;
            }
            try { if (cleanup) releaseMain(); }
            catch (RuntimeException cleanupError) {
                Log.e(TAG, "Failed to release abandoned media operation", cleanupError);
            }
            finally { done.countDown(); }
        }
        synchronized T resultOrAbandon(boolean interrupted) {
            if (!finished) {
                abandoned = true;
                mainHandler.removeCallbacks(this);
                throw new IllegalStateException(interrupted
                        ? "Media operation interrupted; late work will be released"
                        : "Media operation timed out; late work will be released");
            }
            if (failure != null) throw new IllegalStateException(failure);
            return value;
        }
    }

    // Synchronous JNI contract. Never wait indefinitely while NativeActivity's
    // main thread may itself be waiting for a native lifecycle acknowledgement.
    private static <T> T onMain(Callable<T> operation) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            try { return operation.call(); }
            catch (RuntimeException error) { throw error; }
            catch (Exception error) { throw new IllegalStateException(error); }
        }
        MainOperation<T> task = new MainOperation<>(operation);
        if (!mainHandler.post(task)) throw new IllegalStateException("Main queue unavailable");
        boolean interrupted = false;
        try { task.done.await(2, TimeUnit.SECONDS); }
        catch (InterruptedException error) {
            interrupted = true;
            Thread.currentThread().interrupt();
        }
        return task.resultOrAbandon(interrupted);
    }

    /**
     * Initialize the media session. Call once when playback starts.
     *
     * @param activity The current Activity.
     */
    public static void init(Activity activity) {
        onMain(() -> { initMain(activity); return null; });
    }

    private static void initMain(Activity activity) {
        if (sSession != null) return;
        if (activity.isFinishing() || activity.isDestroyed()) throw new IllegalStateException("Activity unavailable");
        sActivity = activity;

        try {
        sSession = new MediaSessionCompat(activity, "GpuiMediaSession");
        sSession.setFlags(
                MediaSessionCompat.FLAG_HANDLES_MEDIA_BUTTONS |
                MediaSessionCompat.FLAG_HANDLES_TRANSPORT_CONTROLS
        );

        sSession.setCallback(new MediaSessionCompat.Callback() {
            @Override
            public void onPlay() {
                Log.i(TAG, "MediaSession callback: onPlay");
                nativeMediaAction("play");
            }

            @Override
            public void onPause() {
                Log.i(TAG, "MediaSession callback: onPause");
                nativeMediaAction("pause");
            }

            @Override
            public void onStop() {
                Log.i(TAG, "MediaSession callback: onStop");
                nativeMediaAction("stop");
            }

            @Override
            public void onSkipToNext() {
                Log.i(TAG, "MediaSession callback: onSkipToNext");
                nativeMediaAction("next");
            }

            @Override
            public void onSkipToPrevious() {
                Log.i(TAG, "MediaSession callback: onSkipToPrevious");
                nativeMediaAction("previous");
            }

            @Override
            public void onSeekTo(long pos) {
                Log.i(TAG, "MediaSession callback: onSeekTo " + pos);
                nativeMediaSeek(pos);
            }
        }, mainHandler);

        sSession.setActive(true);

        // Create notification channel (API 26+)
        sNotificationManager = (NotificationManager) activity.getSystemService(Context.NOTIFICATION_SERVICE);
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel channel = new NotificationChannel(
                    CHANNEL_ID,
                    "Media Playback",
                    NotificationManager.IMPORTANCE_LOW
            );
            channel.setDescription("Controls for audio and video playback");
            channel.setShowBadge(false);
            sNotificationManager.createNotificationChannel(channel);
        }

        Log.i(TAG, "MediaSession initialized");
        } catch (RuntimeException error) {
            releaseMain();
            throw error;
        }
    }

    /**
     * Update the media metadata (title, artist, duration).
     *
     * @param title     Track/video title.
     * @param artist    Artist name (or app name).
     * @param durationMs Duration in milliseconds.
     */
    public static void setMetadata(String title, String artist, long durationMs) {
        onMain(() -> { setMetadataMain(title, artist, durationMs); return null; });
    }
    private static void setMetadataMain(String title, String artist, long durationMs) {
        if (sSession == null) throw new IllegalStateException("Media session not initialized");

        MediaMetadataCompat.Builder builder = new MediaMetadataCompat.Builder()
                .putString(MediaMetadataCompat.METADATA_KEY_TITLE, title != null ? title : "Unknown")
                .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, artist != null ? artist : "GPUI")
                .putLong(MediaMetadataCompat.METADATA_KEY_DURATION, durationMs);

        sSession.setMetadata(builder.build());
        Log.i(TAG, "Metadata updated: " + title + " by " + artist + " (" + durationMs + "ms)");
    }

    /**
     * Update the playback state.
     *
     * @param isPlaying  Whether playback is active.
     * @param positionMs Current playback position in milliseconds.
     * @param speed      Playback speed (1.0 = normal).
     */
    public static void setPlaybackState(boolean isPlaying, long positionMs, float speed) {
        onMain(() -> { setPlaybackStateMain(isPlaying, positionMs, speed); return null; });
    }
    private static void setPlaybackStateMain(boolean isPlaying, long positionMs, float speed) {
        if (sSession == null) throw new IllegalStateException("Media session not initialized");

        int state = isPlaying
                ? PlaybackStateCompat.STATE_PLAYING
                : PlaybackStateCompat.STATE_PAUSED;

        long actions = PlaybackStateCompat.ACTION_PLAY
                | PlaybackStateCompat.ACTION_PAUSE
                | PlaybackStateCompat.ACTION_PLAY_PAUSE
                | PlaybackStateCompat.ACTION_STOP
                | PlaybackStateCompat.ACTION_SEEK_TO
                | PlaybackStateCompat.ACTION_SKIP_TO_NEXT
                | PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS;

        PlaybackStateCompat playbackState = new PlaybackStateCompat.Builder()
                .setActions(actions)
                .setState(state, positionMs, speed)
                .build();

        sSession.setPlaybackState(playbackState);
        updateNotification(isPlaying);
    }

    /**
     * Show or update the media notification with playback controls.
     */
    private static void updateNotification(boolean isPlaying) {
        if (sActivity == null || sSession == null || sNotificationManager == null) return;

        try {
            // Intent to reopen the app when notification is tapped
            Intent openIntent = sActivity.getPackageManager()
                    .getLaunchIntentForPackage(sActivity.getPackageName());
            PendingIntent contentIntent = PendingIntent.getActivity(
                    sActivity, 0, openIntent,
                    PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE
            );

            MediaMetadataCompat metadata = sSession.getController().getMetadata();
            String title = metadata != null
                    ? metadata.getString(MediaMetadataCompat.METADATA_KEY_TITLE)
                    : "GPUI";
            String artist = metadata != null
                    ? metadata.getString(MediaMetadataCompat.METADATA_KEY_ARTIST)
                    : "";

            // Use MediaStyle for system panel integration
            androidx.media.app.NotificationCompat.MediaStyle mediaStyle =
                    new androidx.media.app.NotificationCompat.MediaStyle()
                            .setMediaSession(sSession.getSessionToken())
                            .setShowActionsInCompactView(0, 1, 2);

            NotificationCompat.Builder builder = new NotificationCompat.Builder(sActivity, CHANNEL_ID)
                    .setContentTitle(title)
                    .setContentText(artist)
                    .setSmallIcon(android.R.drawable.ic_media_play)
                    .setContentIntent(contentIntent)
                    .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
                    .setOngoing(isPlaying)
                    .setStyle(mediaStyle)
                    // Previous
                    .addAction(new NotificationCompat.Action(
                            android.R.drawable.ic_media_previous,
                            "Previous",
                            MediaButtonReceiver.buildMediaButtonPendingIntent(
                                    sActivity, PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS)
                    ))
                    // Play / Pause
                    .addAction(new NotificationCompat.Action(
                            isPlaying ? android.R.drawable.ic_media_pause : android.R.drawable.ic_media_play,
                            isPlaying ? "Pause" : "Play",
                            MediaButtonReceiver.buildMediaButtonPendingIntent(
                                    sActivity, PlaybackStateCompat.ACTION_PLAY_PAUSE)
                    ))
                    // Next
                    .addAction(new NotificationCompat.Action(
                            android.R.drawable.ic_media_next,
                            "Next",
                            MediaButtonReceiver.buildMediaButtonPendingIntent(
                                    sActivity, PlaybackStateCompat.ACTION_SKIP_TO_NEXT)
                    ));

            sNotificationManager.notify(NOTIFICATION_ID, builder.build());
        } catch (Exception e) {
            Log.e(TAG, "Failed to update notification", e);
        }
    }

    /**
     * Release the media session and dismiss the notification.
     */
    public static void release() {
        onMain(() -> { releaseMain(); return null; });
    }
    private static void releaseMain() {
        MediaSessionCompat session = sSession;
        NotificationManager notifications = sNotificationManager;
        sSession = null;
        sNotificationManager = null;
        sActivity = null;
        try {
            if (session != null) {
                try {
                    session.setCallback(null, mainHandler);
                    session.setActive(false);
                } finally { session.release(); }
            }
        } finally {
            if (notifications != null) notifications.cancel(NOTIFICATION_ID);
        }
        Log.i(TAG, "MediaSession released");
    }

    /**
     * Get the MediaSessionCompat token for volume routing.
     * Returns null if not initialized.
     */
    public static MediaSessionCompat.Token getSessionToken() {
        return onMain(() -> sSession != null ? sSession.getSessionToken() : null);
    }

    /**
     * JNI callback: notify Rust of a media action from system controls.
     * Actions: "play", "pause", "stop", "next", "previous"
     */
    private static native void nativeMediaAction(String action);

    /**
     * JNI callback: notify Rust of a seek request from system controls.
     */
    private static native void nativeMediaSeek(long positionMs);
}
