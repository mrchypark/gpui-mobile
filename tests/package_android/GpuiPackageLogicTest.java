package dev.gpui.mobile;

/** Plain JVM tests for deterministic logic only; no simulated native OS success. */
public final class GpuiPackageLogicTest {
    private static void equal(Object expected, Object actual) {
        if (!expected.equals(actual)) throw new AssertionError(expected + " != " + actual);
    }
    private static void invalid(Runnable operation) {
        try { operation.run(); } catch (IllegalArgumentException expected) { return; }
        throw new AssertionError("Expected invalid argument");
    }
    public static void main(String[] args) {
        equal("report_2026_.txt", GpuiPickerFiles.safeName("report/2026\\.txt"));
        equal("selected", GpuiPickerFiles.safeName(".."));
        equal("한글.txt", GpuiPickerFiles.safeName("한글.txt"));
        equal("a_b", GpuiPickerFiles.safeName("a\u0000b"));
        equal("[300, 150]", java.util.Arrays.toString(GpuiPickerFiles.targetSize(4000, 2000, 1000, 150)));
        equal("[20, 10]", java.util.Arrays.toString(GpuiPickerFiles.targetSize(20, 10, 1000, 1000)));
        invalid(() -> GpuiPickerFiles.targetSize(20, 10, Double.NaN, 0));
        invalid(() -> GpuiPickerFiles.targetSize(20, 10, 0, 0.5));
        equal("android.permission.READ_EXTERNAL_STORAGE", GpuiPermissions.normalize("android.permission.READ_MEDIA_IMAGES", 32));
        equal("android.permission.READ_MEDIA_IMAGES", GpuiPermissions.normalize("android.permission.READ_MEDIA_IMAGES", 33));
        invalid(() -> GpuiPermissions.normalize("android.permission.READ_EXTERNAL_STORAGE", 33));
        invalid(() -> GpuiPermissions.normalize("", 35));
        String fine = "android.permission.ACCESS_FINE_LOCATION";
        String coarse = "android.permission.ACCESS_COARSE_LOCATION";
        String bluetooth = "android.permission.BLUETOOTH_CONNECT";
        equal("[" + coarse + ", " + fine + "]", java.util.Arrays.toString(GpuiPermissions.runtimePermissions(fine, 31)));
        equal("[android.permission.BLUETOOTH_SCAN, " + bluetooth + "]", java.util.Arrays.toString(GpuiPermissions.runtimePermissions(bluetooth, 31)));
        equal("[android.permission.BLUETOOTH, android.permission.BLUETOOTH_ADMIN, " + coarse + ", " + fine + "]",
            java.util.Arrays.toString(GpuiPermissions.runtimePermissions(bluetooth, 30)));
        // Same aggregation is used by checks and native callback completion.
        for (int coarseStatus : new int[]{0, 1, 2}) {
            for (int fineStatus : new int[]{0, 1, 2}) {
                int expected = fineStatus == 0 ? 0 : coarseStatus == 0 ? 4
                    : coarseStatus == 2 || fineStatus == 2 ? 2 : 1;
                equal(expected, GpuiPermissions.aggregateStatus(fine, 31, new int[]{coarseStatus, fineStatus}));
            }
        }
        equal(0, GpuiPermissions.aggregateStatus(bluetooth, 31, new int[]{0, 0}));
        equal(1, GpuiPermissions.aggregateStatus(bluetooth, 31, new int[]{1, 0}));
        equal(1, GpuiPermissions.aggregateStatus(bluetooth, 31, new int[]{0, 1}));
        equal(2, GpuiPermissions.aggregateStatus(bluetooth, 31, new int[]{2, 0}));
        equal(1, GpuiPermissions.aggregateStatus(bluetooth, 30, new int[]{0, 0, 0, 1}));
        equal(0, GpuiPermissions.aggregateStatus(bluetooth, 30, new int[]{0, 0, 0, 0}));
        equal(0, GpuiPermissions.aggregateStatus(bluetooth, 28, new int[]{0, 0, 0, 1}));
        equal(1, GpuiPermissions.aggregateStatus(bluetooth, 28, new int[]{0, 0, 1, 1}));
        equal(1, GpuiPermissions.aggregateStatus("android.permission.ACCESS_BACKGROUND_LOCATION", 28, new int[]{0, 1}));
        equal("[android.permission.ACCESS_BACKGROUND_LOCATION]", java.util.Arrays.toString(
            GpuiPermissions.runtimePermissions("android.permission.ACCESS_BACKGROUND_LOCATION", 31)));
        invalid(() -> GpuiPermissions.aggregateStatus(bluetooth, 31, new int[]{0}));
        System.out.println("PASS: picker filename/bounds and permission normalization regressions");
    }
}
