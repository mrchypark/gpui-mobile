package dev.gpui.mobile;

import android.app.Activity;
import android.content.Context;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.PowerManager;

/** Thermal events are queued by JNI; GPUI callbacks never execute on Java main. */
public final class GpuiThermal {
    private GpuiThermal() {}
    private static Session active;
    private static long newestGeneration;
    private static native void nativeStatusChanged(long generation, int status);
    private static native void nativeListenerStopped(long generation);

    private interface Session {
        Activity host();
        void close();
    }
    private static void mainThread() {
        if (Looper.myLooper() != Looper.getMainLooper())
            throw new IllegalStateException("Thermal lifecycle requires Java main");
    }
    public static boolean register(Activity activity, long generation) {
        mainThread();
        if (generation <= 0) throw new IllegalArgumentException("Invalid thermal generation");
        if (generation == newestGeneration && active != null && active.host() == activity) return true;
        if (generation <= newestGeneration) { nativeListenerStopped(generation); return false; }
        newestGeneration = generation;
        if (activity == null || activity.isDestroyed() || activity.isFinishing()) {
            nativeListenerStopped(generation); return false;
        }
        if (active != null) { Session old = active; active = null; old.close(); }
        if (Build.VERSION.SDK_INT < 29) { nativeListenerStopped(generation); return false; }
        Api29 session = null;
        try { session = new Api29(activity, generation); active = session; session.start(); return true; }
        catch (RuntimeException failure) {
            active = null;
            nativeListenerStopped(generation);
            try { if (session != null) session.close(); } catch (RuntimeException cleanup) { failure.addSuppressed(cleanup); }
            throw failure;
        }
    }
    public static void unregister(Activity activity) {
        mainThread();
        if (active != null && active.host() == activity) {
            Session old = active; active = null; old.close();
        }
    }
    public static int currentStatus(Activity activity) {
        if (Build.VERSION.SDK_INT < 29 || activity == null) return -1;
        return Api29.currentStatus(activity);
    }
    static boolean validStatus(int status) { return status >= 0 && status <= 6; }

    private static final class Api29 implements Session {
        private final Activity activity;
        private final long generation;
        private final PowerManager power;
        private final Handler handler = new Handler(Looper.getMainLooper());
        private final PowerManager.OnThermalStatusChangedListener listener;
        private boolean registered;
        Api29(Activity activity, long generation) {
            this.activity = activity;
            this.generation = generation;
            power = (PowerManager) activity.getSystemService(Context.POWER_SERVICE);
            if (power == null) throw new IllegalStateException("PowerManager unavailable");
            listener = status -> {
                if (active == this && !activity.isDestroyed() && validStatus(status))
                    nativeStatusChanged(this.generation, status);
            };
        }
        void start() {
            // Set before registration so cleanup also covers partial registration failure.
            registered = true;
            power.addThermalStatusListener(command -> {
                if (!handler.post(command)) throw new java.util.concurrent.RejectedExecutionException("Main looper stopped");
            }, listener);
        }
        public Activity host() { return activity; }
        public void close() {
            // Invalidate native queued/in-flight events before OS deregistration.
            nativeListenerStopped(generation);
            if (registered) { registered = false; power.removeThermalStatusListener(listener); }
        }
        static int currentStatus(Activity activity) {
            PowerManager power = (PowerManager) activity.getSystemService(Context.POWER_SERVICE);
            if (power == null) return -1;
            int status = power.getCurrentThermalStatus();
            return validStatus(status) ? status : -1;
        }
    }
}
