package org.gpui.mobile.ble;

/** Invoke after loading the app's native library, from an application Java thread.
 * The matching btleplug Java library must be packaged in the APK.
 * Failure raises a Java exception; initialization never silently succeeds.
 */
public final class Ble {
    private Ble() {}
    public static native void initialize();
    /** Called by the checksum-pinned Android ScanCallback bridge. */
    public static void scanFailed(int code) { nativeScanFailed(code); }
    private static native void nativeScanFailed(int code);
}
