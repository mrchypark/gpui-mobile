package dev.gpui.mobile;

import android.app.Activity;
import android.media.MediaPlayer;
import android.media.PlaybackParams;
import android.util.SparseArray;

/** Serialized player operations; native errors reach the Rust Result through JNI. */
public final class GpuiAudio {
    private static final class Player {
        final MediaPlayer nativePlayer = new MediaPlayer();
        int state; // 0 idle, 1 loading, 2 ready, 3 playing, 4 paused, 5 completed, -1 error
        float speed = 1, volume = 1;
        boolean looping;
        GpuiMediaLoad load;
        long generation;
    }
    private static final SparseArray<Player> players = new SparseArray<>();
    private static int nextId = 1;
    private static Player player(int id) {
        Player p = players.get(id);
        if (p == null) throw new IllegalStateException("Audio player not found: " + id);
        return p;
    }
    private static void ready(Player p) {
        if (p.state < 2) throw new IllegalStateException("Audio source is not ready");
    }
    public static synchronized int create(Activity activity) {
        Player p = new Player();
        int id = nextId++;
        players.put(id, p);
        return id;
    }
    public static long setUrl(Activity activity, int id, String url) {
        if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) {
            throw new IllegalStateException("Load audio from a worker thread");
        }
        final Player p;
        final GpuiMediaLoad load = new GpuiMediaLoad();
        final long generation;
        synchronized (GpuiAudio.class) {
            p = player(id);
            if (p.load != null) throw new IllegalStateException("Audio source is loading");
            generation = ++p.generation;
            p.load = load; p.state = 1;
            try {
                p.nativePlayer.reset();
                p.nativePlayer.setOnPreparedListener(mp -> load.complete(null));
                p.nativePlayer.setOnCompletionListener(mp -> {
                    synchronized (GpuiAudio.class) { if (p.generation == generation) p.state = 5; }
                });
                p.nativePlayer.setOnErrorListener((mp, what, extra) -> {
                    synchronized (GpuiAudio.class) { if (p.generation == generation) p.state = -1; }
                    load.complete(new IllegalStateException("Audio error " + what + "/" + extra)); return true;
                });
                android.net.Uri uri = android.net.Uri.parse(url);
                String scheme = uri.getScheme();
                if (scheme == null || "http".equals(scheme) || "https".equals(scheme)) p.nativePlayer.setDataSource(url);
                else p.nativePlayer.setDataSource(activity, uri);
                p.nativePlayer.prepareAsync();
            } catch (Exception error) {
                p.load = null; p.state = -1;
                throw new IllegalStateException("Audio loading failed", error);
            }
        }
        try {
            load.await();
            synchronized (GpuiAudio.class) {
                if (players.get(id) != p || p.load != load) throw new IllegalStateException("Audio load cancelled");
                p.nativePlayer.setLooping(p.looping);
                p.nativePlayer.setVolume(p.volume, p.volume);
                p.state = 2;
                return p.nativePlayer.getDuration();
            }
        } catch (RuntimeException error) {
            synchronized (GpuiAudio.class) {
                if (players.get(id) == p && p.load == load) { p.nativePlayer.reset(); p.state = -1; }
            }
            throw error;
        } finally {
            synchronized (GpuiAudio.class) { if (p.load == load) p.load = null; }
        }
    }
    public static synchronized void play(int id) {
        Player p = player(id); ready(p);
        p.nativePlayer.setPlaybackParams(new PlaybackParams().allowDefaults().setSpeed(p.speed));
        p.nativePlayer.start(); p.state = 3;
    }
    public static synchronized void pause(int id) {
        Player p = player(id); ready(p);
        if (p.nativePlayer.isPlaying()) p.nativePlayer.pause();
        p.state = 4;
    }
    public static synchronized void stop(int id) {
        Player p = player(id); ready(p);
        if (p.nativePlayer.isPlaying()) p.nativePlayer.pause();
        p.nativePlayer.seekTo(0L, MediaPlayer.SEEK_CLOSEST); p.state = 2;
    }
    public static synchronized void seek(int id, long ms) {
        Player p = player(id); ready(p);
        if (ms < 0) throw new IllegalArgumentException("Negative seek");
        p.nativePlayer.seekTo(ms, MediaPlayer.SEEK_CLOSEST);
    }
    public static synchronized void setVolume(int id, float volume) {
        if (!Float.isFinite(volume) || volume < 0 || volume > 1) throw new IllegalArgumentException("Invalid volume");
        Player p = player(id);
        if (p.load != null) throw new IllegalStateException("Audio source is loading");
        p.nativePlayer.setVolume(volume, volume); p.volume = volume;
    }
    public static synchronized void setSpeed(int id, float speed) {
        if (!Float.isFinite(speed) || speed <= 0) throw new IllegalArgumentException("Invalid speed");
        Player p = player(id);
        if (p.load != null) throw new IllegalStateException("Audio source is loading");
        if (p.state == 3) p.nativePlayer.setPlaybackParams(new PlaybackParams().allowDefaults().setSpeed(speed));
        p.speed = speed;
    }
    public static synchronized void setLooping(int id, boolean looping) {
        Player p = player(id);
        if (p.load != null) throw new IllegalStateException("Audio source is loading");
        p.nativePlayer.setLooping(looping); p.looping = looping;
    }
    public static synchronized long getPosition(int id) {
        Player p = player(id);
        if (p.state == 0) return 0;
        ready(p); return p.nativePlayer.getCurrentPosition();
    }
    public static synchronized long getDuration(int id) {
        Player p = player(id);
        if (p.state == 0) return 0;
        ready(p); return p.nativePlayer.getDuration();
    }
    public static synchronized boolean isPlaying(int id) { return getState(id) == 3; }
    public static synchronized int getState(int id) {
        Player p = player(id);
        if (p.state == -1) throw new IllegalStateException("Native audio playback failed");
        return p.state;
    }
    public static synchronized void dispose(int id) {
        Player p = players.get(id);
        if (p == null) return;
        players.remove(id); p.generation++;
        if (p.load != null) p.load.complete(new IllegalStateException("Audio player disposed during load"));
        p.nativePlayer.release();
    }
    private GpuiAudio() {}
}
