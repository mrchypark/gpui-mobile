package dev.gpui.mobile;

import android.app.Activity;
import dev.gpui.mobile.example.BuildConfig;

/** Explicit debug-only developer action; never runs for an ordinary launch. */
final class GpuiServiceProbe {
    private static volatile boolean watchActive;
    private static int probeAvailability = -1;
    private static boolean probeAvailable() {
        if (!BuildConfig.DEBUG || !BuildConfig.BLE_ENABLED) return false;
        if (probeAvailability < 0) {
            try { probeAvailability = nativeAvailable() ? 1 : 0; }
            catch (UnsatisfiedLinkError unavailable) { probeAvailability = 0; }
        }
        return probeAvailability == 1;
    }
    static int mode(String value) {
        if ("ble".equals(value)) return 1;
        if ("gps".equals(value)) return 2;
        if ("gps-network".equals(value)) return 3;
        if ("gps-watch".equals(value)) return 4;
        if ("gatt-echo".equals(value)) return 5;
        if ("gatt-background".equals(value)) return 6;
        if ("gatt-survival".equals(value)) return 7;
        return 0;
    }
    static void resumed(Activity activity) {
        if (!probeAvailable()) return;
        nativeForeground(true);
        String value = activity.getIntent().getStringExtra("gpui.service_probe");
        activity.getIntent().removeExtra("gpui.service_probe");
        if ("ble-beacons".equals(value)) { scheduleRun(activity, 4); return; }
        int mode = mode(value);
        if (mode == 4) {
            if (!watchActive) {
                watchActive = true;
                if (!nativeLocationWatch()) watchActive = false;
            }
        }
        else if (mode != 0) scheduleRun(activity, mode);
    }
    private static void scheduleRun(Activity activity, int mode) {
        // GPUI activation can briefly pause the host during initial launch.
        new android.os.Handler(android.os.Looper.getMainLooper()).postDelayed(() -> {
            if (!activity.isFinishing() && !activity.isDestroyed()) nativeRun(mode);
        }, 1000);
    }
    static void paused() {
        if (probeAvailable()) nativeForeground(false);
    }
    private static native boolean nativeAvailable();
    private static native void nativeForeground(boolean resumed);
    private static native void nativeRun(int mode);
    private static native boolean nativeLocationWatch();
    public static boolean moveWatchToBackground(long token) {
        if (!probeAvailable() || !watchActive) return false;
        return GpuiLocation.probeMoveToBackground(token, () -> watchActive);
    }
    public static void finishLocationWatch() { watchActive = false; }
    public static int locationState(long token) {
        return probeAvailable() ? GpuiLocation.probeState(token) : -1;
    }
    public static long locationPhase(long token, long timestamp, long boundary, boolean resumed) {
        return probeAvailable() ? GpuiLocation.probePhase(token, timestamp, boundary, resumed) : -1;
    }
}
