package dev.gpui.mobile;
public final class AuthSessionTest {
    public static void main(String[] args) {
        checkAvailability(androidx.biometric.BiometricManager.BIOMETRIC_SUCCESS, 0);
        checkAvailability(androidx.biometric.BiometricManager.BIOMETRIC_ERROR_NONE_ENROLLED, 3);
        checkAvailability(androidx.biometric.BiometricManager.BIOMETRIC_ERROR_HW_UNAVAILABLE, 2);
        checkAvailability(androidx.biometric.BiometricManager.BIOMETRIC_ERROR_NO_HARDWARE, 2);
        checkAvailability(-999, 7);
        GpuiLocalAuth.Session old = new GpuiLocalAuth.Session(1, "old");
        GpuiLocalAuth.Session next = new GpuiLocalAuth.Session(2, "next");
        old.complete(4);
        old.complete(0); // late success must not replace cancellation
        assert old.result.get() == 4 && old.latch.getCount() == 0;
        assert next.result.get() == -1 && next.latch.getCount() == 1;
        next.complete(0);
        old.complete(7); // old lifecycle/timeout cannot affect next request
        assert next.result.get() == 0 && next.latch.getCount() == 0;
        System.out.println("PASS request-local authentication completion and stale callback isolation");
    }
    private static void checkAvailability(int status, int expected) {
        if (GpuiLocalAuth.availabilityResult(status) != expected) {
            throw new AssertionError("Incorrect availability result: " + status);
        }
    }
}
