package dev.gpui.mobile;

import android.app.Activity;
import android.content.Intent;
import androidx.biometric.BiometricManager;

import java.util.ArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Biometric authentication helper for GPUI.
 *
 * <p>Uses AndroidX BiometricManager for availability checks and launches
 * {@link GpuiAuthActivity} (a transparent FragmentActivity) to show the
 * BiometricPrompt dialog.</p>
 *
 * <p>The {@code authenticate} method blocks the calling thread via a
 * {@link CountDownLatch} until the prompt completes.</p>
 */
public final class GpuiLocalAuth {

    private static Session active;
    private static long nextId;
    static final class Session {
        final long id;
        final String reason;
        final CountDownLatch latch = new CountDownLatch(1);
        final AtomicInteger result = new AtomicInteger(-1);
        volatile java.lang.ref.WeakReference<GpuiAuthActivity> owner = new java.lang.ref.WeakReference<>(null);
        Session(long id, String reason) { this.id = id; this.reason = reason; }
        void complete(int value) { if (result.compareAndSet(-1, value)) latch.countDown(); }
    }
    static synchronized Session session(long id) {
        return active != null && active.id == id && active.result.get() == -1 ? active : null;
    }

    // Result codes (must match Rust int_to_auth_result)
    // 0 = success, 1 = failed, 2 = not_available, 3 = not_enrolled,
    // 4 = cancelled, 5 = passcode_not_set, 6 = lockout, 7 = other

    /**
     * Check if the device has biometric hardware.
     *
     * @return {@code true} if biometric hardware is present (even if not enrolled).
     */
    public static boolean isDeviceSupported(Activity activity) {
        int status = availability(activity);
        return status == BiometricManager.BIOMETRIC_SUCCESS
            || status == BiometricManager.BIOMETRIC_ERROR_NONE_ENROLLED
            || status == BiometricManager.BIOMETRIC_ERROR_HW_UNAVAILABLE;
    }

    /**
     * Check if biometrics are enrolled and ready to use.
     *
     * @return {@code true} if the user can authenticate right now.
     */
    public static boolean canAuthenticate(Activity activity) {
        return availability(activity) == BiometricManager.BIOMETRIC_SUCCESS;
    }

    private static int availability(Activity activity) {
        return BiometricManager.from(activity).canAuthenticate(
            BiometricManager.Authenticators.BIOMETRIC_WEAK);
    }

    static int availabilityResult(int status) {
        if (status == BiometricManager.BIOMETRIC_SUCCESS) return 0;
        if (status == BiometricManager.BIOMETRIC_ERROR_NONE_ENROLLED) return 3;
        if (status == BiometricManager.BIOMETRIC_ERROR_NO_HARDWARE
                || status == BiometricManager.BIOMETRIC_ERROR_HW_UNAVAILABLE) return 2;
        return 7;
    }

    /**
     * Get pipe-delimited list of available biometric types.
     *
     * @return e.g. "fingerprint|face" or "" if none.
     */
    public static String getAvailableBiometrics(Activity activity) {
        ArrayList<String> types = new ArrayList<>();
        if (activity.getPackageManager().hasSystemFeature("android.hardware.fingerprint")) {
            types.add("fingerprint");
        }
        if (activity.getPackageManager().hasSystemFeature("android.hardware.biometrics.face")) {
            types.add("face");
        }
        if (activity.getPackageManager().hasSystemFeature("android.hardware.biometrics.iris")) {
            types.add("iris");
        }
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < types.size(); i++) {
            if (i > 0) sb.append("|");
            sb.append(types.get(i));
        }
        return sb.toString();
    }

    /**
     * Authenticate the user with biometrics. Blocks until complete.
     *
     * <p>Launches {@link GpuiAuthActivity} which shows a BiometricPrompt.
     * The calling thread blocks on a {@link CountDownLatch} until the
     * prompt callback fires.</p>
     *
     * @param activity the current Activity context
     * @param reason   the reason string shown to the user
     * @return result code (0=success, 1=failed, 2=not_available, etc.)
     */
    public static int authenticate(Activity activity, String reason) {
        if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) {
            throw new IllegalStateException("Call authenticate from a worker thread");
        }
        if (activity == null || activity.isFinishing() || activity.isDestroyed()) return 4;
        if (reason == null || reason.trim().isEmpty()) throw new IllegalArgumentException("Authentication reason must not be empty");
        int available = availabilityResult(availability(activity));
        if (available != 0) return available;

        final Session request;
        synchronized (GpuiLocalAuth.class) {
            if (active != null) throw new IllegalStateException("Authentication already in progress");
            request = new Session(++nextId, reason);
            active = request;
        }

        try {
            activity.runOnUiThread(() -> {
                if (session(request.id) != request) return;
                try {
                    if (activity.isFinishing() || activity.isDestroyed()) { request.complete(4); return; }
                    Intent intent = new Intent(activity, GpuiAuthActivity.class);
                    intent.putExtra("gpui.auth.request", request.id);
                    activity.startActivity(intent);
                } catch (RuntimeException error) { request.complete(7); }
            });
            if (!request.latch.await(120, java.util.concurrent.TimeUnit.SECONDS)) request.complete(7);
            return request.result.get();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            request.complete(4);
            return 4;
        } finally {
            synchronized (GpuiLocalAuth.class) { if (active == request) active = null; }
            activity.runOnUiThread(() -> {
                GpuiAuthActivity owner = request.owner.get();
                if (owner != null) owner.cancelAndFinish();
            });
        }
    }

    private GpuiLocalAuth() {}
}
