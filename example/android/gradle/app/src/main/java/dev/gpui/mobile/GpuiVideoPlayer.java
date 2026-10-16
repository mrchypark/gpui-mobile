package dev.gpui.mobile;

import android.app.Activity;
import android.media.MediaPlayer;
import android.media.PlaybackParams;
import android.net.Uri;
import android.os.Build;
import android.util.SparseArray;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.ViewGroup;
import android.widget.FrameLayout;

import java.util.concurrent.CountDownLatch;

/**
 * Java helper for video playback using {@link MediaPlayer}.
 *
 * <p>All public methods are {@code static} and called from Rust via JNI.
 * Player instances are stored in a {@link SparseArray} keyed by integer ID.</p>
 *
 * <p>Uses {@link SurfaceView} instead of TextureView because NativeActivity
 * does not provide hardware acceleration for views added via addContentView().
 * TextureView requires HW accel and silently renders nothing without it.
 * SurfaceView has its own compositor surface and works without HW accel.</p>
 */
public final class GpuiVideoPlayer {

    private static final String TAG = "GpuiVideoPlayer";

    private static final SparseArray<MediaPlayer> sPlayers = new SparseArray<>();
    private static final SparseArray<SurfaceView> sSurfaces = new SparseArray<>();
    private static int sNextId = 1;
    private static final Object sLock = new Object();
    private static final SparseArray<GpuiMediaLoad> sLoads = new SparseArray<>();
    private static final SparseArray<Options> sOptions = new SparseArray<>();
    private static final class Options { float speed = 1, volume = 1; boolean looping; }

    /**
     * Create a new MediaPlayer instance.
     *
     * @param activity The current Activity.
     * @return A positive player ID, or -1 on failure.
     */
    public static int create(Activity activity) {
        try {
            MediaPlayer mp = new MediaPlayer();
            synchronized (sLock) {
                int id = sNextId++;
                sPlayers.put(id, mp);
                sOptions.put(id, new Options());
                return id;
            }
        } catch (Exception e) {
            android.util.Log.e(TAG, "create failed", e);
            return -1;
        }
    }

    /**
     * Set a URL as the data source and prepare synchronously.
     *
     * @param activity The current Activity.
     * @param id       Player ID.
     * @param url      URL to play.
     * @return "duration|width|height" on success, or null on failure.
     */
    public static String setUrl(Activity activity, int id, String url) { return load(activity, id, url, false); }
    public static String setFilePath(Activity activity, int id, String path) { return load(activity, id, path, true); }

    private static String load(Activity activity, int id, String source, boolean file) {
        if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) {
            throw new IllegalStateException("Load media from a worker thread");
        }
        GpuiMediaLoad load = new GpuiMediaLoad();
        MediaPlayer mp;
        synchronized (sLock) {
            mp = requirePlayer(id);
            sLoads.put(id, load);
            try {
                mp.reset();
                mp.setOnPreparedListener(player -> load.complete(null));
                mp.setOnErrorListener((player, what, extra) -> {
                    load.complete(new IllegalStateException("Video error " + what + "/" + extra)); return true;
                });
                Uri uri = Uri.parse(source);
                String scheme = uri.getScheme();
                if (file || "http".equals(scheme) || "https".equals(scheme)) mp.setDataSource(source);
                else mp.setDataSource(activity, uri);
                mp.prepareAsync();
            } catch (Exception error) {
                sLoads.remove(id);
                throw new IllegalStateException("Video preparation failed", error);
            }
        }
        try {
            load.await(); // No registry/native-operation lock while waiting for the UI callback.
            synchronized (sLock) {
                if (sPlayers.get(id) != mp || sLoads.get(id) != load) throw new IllegalStateException("Video load cancelled");
                Options options = sOptions.get(id);
                mp.setLooping(options.looping);
                mp.setVolume(options.volume, options.volume);
                return mp.getDuration() + "|" + mp.getVideoWidth() + "|" + mp.getVideoHeight();
            }
        } catch (RuntimeException error) {
            synchronized (sLock) { if (sPlayers.get(id) == mp) mp.reset(); }
            throw error;
        } finally {
            synchronized (sLock) { if (sLoads.get(id) == load) sLoads.remove(id); }
        }
    }

    private interface Operation<T> { T run(MediaPlayer player); }
    private static MediaPlayer requirePlayer(int id) {
        MediaPlayer player = sPlayers.get(id);
        if (player == null) throw new IllegalStateException("Video player not found");
        if (sLoads.get(id) != null) throw new IllegalStateException("Video source is loading");
        return player;
    }
    private static <T> T withPlayer(int id, Operation<T> operation) {
        synchronized (sLock) { return operation.run(requirePlayer(id)); }
    }
    public static void play(int id) {
        withPlayer(id, mp -> {
            mp.setPlaybackParams(new PlaybackParams().allowDefaults().setSpeed(sOptions.get(id).speed));
            mp.start(); return null;
        });
    }
    public static void pause(int id) { withPlayer(id, mp -> { if (mp.isPlaying()) mp.pause(); return null; }); }
    public static void seek(int id, long ms) {
        if (ms < 0) throw new IllegalArgumentException("Negative seek");
        withPlayer(id, mp -> { mp.seekTo(ms, MediaPlayer.SEEK_CLOSEST); return null; });
    }
    public static void setVolume(int id, float volume) {
        if (!Float.isFinite(volume) || volume < 0 || volume > 1) throw new IllegalArgumentException("Invalid volume");
        withPlayer(id, mp -> { mp.setVolume(volume, volume); sOptions.get(id).volume = volume; return null; });
    }
    public static void setSpeed(int id, float speed) {
        if (!Float.isFinite(speed) || speed <= 0) throw new IllegalArgumentException("Invalid speed");
        withPlayer(id, mp -> {
            if (mp.isPlaying()) mp.setPlaybackParams(new PlaybackParams().allowDefaults().setSpeed(speed));
            sOptions.get(id).speed = speed; return null;
        });
    }
    public static void setLooping(int id, boolean looping) {
        withPlayer(id, mp -> { mp.setLooping(looping); sOptions.get(id).looping = looping; return null; });
    }
    public static long getPosition(int id) { return withPlayer(id, mp -> (long)mp.getCurrentPosition()); }
    public static long getDuration(int id) { return withPlayer(id, mp -> (long)mp.getDuration()); }
    public static int getWidth(int id) { return withPlayer(id, MediaPlayer::getVideoWidth); }
    public static int getHeight(int id) { return withPlayer(id, MediaPlayer::getVideoHeight); }
    public static boolean isPlaying(int id) { return withPlayer(id, MediaPlayer::isPlaying); }

    public static void showSurface(Activity activity, int id, int x, int y, int width, int height) {
        MediaPlayer mp;
        synchronized (sLock) {
            mp = sPlayers.get(id);
        }
        if (mp == null) return;

        final MediaPlayer fmp = mp;
        final CountDownLatch latch = new CountDownLatch(1);

        activity.runOnUiThread(() -> {
            try {
                // Remove existing surface if any
                hideSurfaceInternal(id);

                SurfaceView sv = new SurfaceView(activity);
                // Render above NativeActivity's own SurfaceView (wgpu/Vulkan).
                sv.setZOrderOnTop(true);

                FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(width, height);
                params.leftMargin = x;
                params.topMargin = y;
                activity.addContentView(sv, params);

                synchronized (sLock) {
                    if (sPlayers.get(id) != fmp) {
                        ((ViewGroup)sv.getParent()).removeView(sv);
                        latch.countDown();
                        return;
                    }
                    sSurfaces.put(id, sv);
                }

                sv.getHolder().addCallback(new SurfaceHolder.Callback() {
                    @Override
                    public void surfaceCreated(SurfaceHolder holder) {
                        updateSurface(id, fmp, sv, holder.getSurface());
                        latch.countDown();
                    }

                    @Override
                    public void surfaceChanged(SurfaceHolder holder, int format, int w, int h) {}

                    @Override
                    public void surfaceDestroyed(SurfaceHolder holder) {
                        updateSurface(id, fmp, sv, null);
                    }
                });

                // If surface is already valid
                if (sv.getHolder().getSurface().isValid()) {
                    updateSurface(id, fmp, sv, sv.getHolder().getSurface());
                    latch.countDown();
                }
            } catch (Exception e) {
                android.util.Log.e(TAG, "showSurface failed", e);
                latch.countDown();
            }
        });

        try {
            latch.await(java.util.concurrent.TimeUnit.SECONDS.toMillis(3),
                        java.util.concurrent.TimeUnit.MILLISECONDS);
        } catch (InterruptedException ignored) {}
    }

    /**
     * Hide (remove) the native video surface overlay.
     *
     * @param activity The current Activity.
     * @param id       Player ID.
     */
    public static void hideSurface(Activity activity, int id) {
        final CountDownLatch latch = new CountDownLatch(1);
        activity.runOnUiThread(() -> {
            hideSurfaceInternal(id);
            latch.countDown();
        });
        try {
            latch.await(java.util.concurrent.TimeUnit.SECONDS.toMillis(2),
                        java.util.concurrent.TimeUnit.MILLISECONDS);
        } catch (InterruptedException ignored) {}
    }

    private static void hideSurfaceInternal(int id) {
        SurfaceView sv;
        synchronized (sLock) {
            sv = sSurfaces.get(id);
            MediaPlayer player = sPlayers.get(id);
            if (sv != null && player != null) updateSurface(id, player, sv, null);
            sSurfaces.remove(id);
        }
        if (sv != null) {
            Runnable remove = () -> {
                ViewGroup parent = (ViewGroup) sv.getParent();
                if (parent != null) parent.removeView(sv);
            };
            if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) remove.run();
            else new android.os.Handler(android.os.Looper.getMainLooper()).post(remove);
        }
    }

    /**
     * Release and remove the player.
     */
    public static void dispose(int id) {
        MediaPlayer mp;
        synchronized (sLock) {
            // No new surface can register between removal and player release.
            hideSurfaceInternal(id);
            mp = sPlayers.get(id);
            sPlayers.remove(id);
            sOptions.remove(id);
            GpuiMediaLoad load = sLoads.get(id);
            sLoads.remove(id);
            if (load != null) load.complete(new IllegalStateException("Video player disposed during load"));
        if (mp != null) {
            try {
                mp.setSurface(null);
            } catch (Exception ignored) {
            }
            try {
                mp.stop();
            } catch (Exception ignored) {
            }
            try {
                mp.release();
            } catch (Exception ignored) {
            }
        }
        }
    }

    /**
     * Create a SurfaceView configured for video playback.
     *
     * <p>Uses SurfaceView with setZOrderOnTop(true) so the video surface
     * renders above NativeActivity's own rendering surface (wgpu/Vulkan).
     * TextureView cannot be used here because NativeActivity does not
     * provide hardware acceleration for overlaid views.</p>
     *
     * <p>The view is NOT added to the hierarchy — the caller
     * (GpuiPlatformView) handles that.</p>
     *
     * @param activity The current Activity.
     * @param id       Player ID.
     * @return A configured SurfaceView, or an empty FrameLayout if the player is not found.
     */
    public static android.view.View createVideoSurface(Activity activity, int id) {
        MediaPlayer mp;
        synchronized (sLock) {
            mp = sPlayers.get(id);
        }
        if (mp == null) {
            android.util.Log.w(TAG, "createVideoSurface: player " + id + " not found");
            return new FrameLayout(activity);
        }

        final MediaPlayer fmp = mp;
        SurfaceView sv = new SurfaceView(activity);
        // Render above NativeActivity's own SurfaceView.
        sv.setZOrderOnTop(true);

        synchronized (sLock) {
            if (sPlayers.get(id) != fmp) return new FrameLayout(activity);
            sSurfaces.put(id, sv);
        }

        sv.getHolder().addCallback(new SurfaceHolder.Callback() {
            @Override
            public void surfaceCreated(SurfaceHolder holder) {
                android.util.Log.i(TAG, "createVideoSurface: surfaceCreated for player " + id);
                updateSurface(id, fmp, sv, holder.getSurface());
            }

            @Override
            public void surfaceChanged(SurfaceHolder holder, int format, int w, int h) {
                android.util.Log.i(TAG, "createVideoSurface: surfaceChanged " + w + "x" + h);
            }

            @Override
            public void surfaceDestroyed(SurfaceHolder holder) {
                android.util.Log.i(TAG, "createVideoSurface: surfaceDestroyed for player " + id);
                updateSurface(id, fmp, sv, null);
            }
        });

        // If surface is already valid (shouldn't happen before layout, but be safe)
        if (sv.getHolder().getSurface().isValid()) {
            updateSurface(id, fmp, sv, sv.getHolder().getSurface());
        }

        android.util.Log.i(TAG, "createVideoSurface: SurfaceView created for player " + id);
        return sv;
    }

    /**
     * Attach a surface to the MediaPlayer, then re-seek to force a video frame
     * to render. Without the re-seek, frames decoded before the surface was
     * attached are discarded and the view stays black.
     */
    static boolean ownsSurface(Object registeredPlayer, Object expectedPlayer,
            Object registeredSurface, Object expectedSurface) {
        return expectedPlayer != null && expectedSurface != null
            && registeredPlayer == expectedPlayer && registeredSurface == expectedSurface;
    }

    private static void updateSurface(int id, MediaPlayer mp, SurfaceView owner, Surface surface) {
        synchronized (sLock) {
        if (!ownsSurface(sPlayers.get(id), mp, sSurfaces.get(id), owner)) return;
        try {
            mp.setSurface(surface);
            // Loading/reset and disposal use this same mutex. Do not seek a
            // player while its asynchronous preparation is still in flight.
            if (surface == null || sLoads.get(id) != null) return;
            android.util.Log.i(TAG, "attachSurfaceToPlayer: surface attached, isPlaying=" + mp.isPlaying()
                    + " pos=" + mp.getCurrentPosition() + " dur=" + mp.getDuration()
                    + " w=" + mp.getVideoWidth() + " h=" + mp.getVideoHeight());
            // Re-seek to current position to force the decoder to output a
            // frame to the newly attached surface.
            int pos = mp.getCurrentPosition();
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                mp.seekTo(pos, MediaPlayer.SEEK_CLOSEST);
            } else {
                mp.seekTo(pos);
            }
        } catch (Exception e) {
            android.util.Log.e(TAG, "attachSurfaceToPlayer failed", e);
        }
        }
    }

    // Prevent instantiation.
    private GpuiVideoPlayer() {}
}
