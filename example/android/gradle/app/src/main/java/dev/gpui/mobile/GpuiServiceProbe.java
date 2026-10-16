package dev.gpui.mobile;

import android.app.Activity;
import dev.gpui.mobile.example.BuildConfig;

/** Explicit debug-only developer action; never runs for an ordinary launch. */
final class GpuiServiceProbe {
    private static volatile boolean watchActive;
    static int mode(String value) {
        if ("ble".equals(value)) return 1;
        if ("gps".equals(value)) return 2;
        if ("gps-network".equals(value)) return 3;
        if ("gps-watch".equals(value)) return 4;
        return 0;
    }
    static void resumed(Activity activity) {
        if (!BuildConfig.DEBUG || !BuildConfig.BLE_ENABLED) return;
        nativeForeground(true);
        String value = activity.getIntent().getStringExtra("gpui.service_probe");
        activity.getIntent().removeExtra("gpui.service_probe");
        if ("ble-beacons".equals(value)) { nativeRun(4); return; }
        int mode = mode(value);
        if (mode == 4) {
            if (!watchActive) {
                watchActive = true;
                if (!nativeLocationWatch()) watchActive = false;
            }
        }
        else if (mode != 0) nativeRun(mode);
    }
    static void paused() {
        if (BuildConfig.DEBUG && BuildConfig.BLE_ENABLED) nativeForeground(false);
    }
    private static native void nativeForeground(boolean resumed);
    private static native void nativeRun(int mode);
    private static native boolean nativeLocationWatch();
    public static boolean moveWatchToBackground(long token) {
        if (!BuildConfig.DEBUG || !BuildConfig.BLE_ENABLED || !watchActive) return false;
        return GpuiLocation.probeMoveToBackground(token, () -> watchActive);
    }
    public static void finishLocationWatch() { watchActive = false; }
    public static int locationState(long token) {
        return BuildConfig.DEBUG && BuildConfig.BLE_ENABLED ? GpuiLocation.probeState(token) : -1;
    }
    public static long locationPhase(long token, long timestamp, long boundary, boolean resumed) {
        return BuildConfig.DEBUG && BuildConfig.BLE_ENABLED ? GpuiLocation.probePhase(token, timestamp, boundary, resumed) : -1;
    }
}
