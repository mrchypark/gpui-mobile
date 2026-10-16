package dev.gpui.mobile;

import android.app.Activity;
import android.app.NotificationManager;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.location.LocationManager;
import android.net.Uri;
import android.os.Build;
import android.provider.Settings;

public final class GpuiPermissions {
    static final int GRANTED = 0, DENIED = 1, PERMANENTLY_DENIED = 2, LIMITED = 4;
    static final String HISTORY = "gpui-permission-history";

    static String normalize(String permission, int sdk) {
        if (permission == null || permission.isEmpty()) throw new IllegalArgumentException("Permission is not applicable on Android");
        if (permission.startsWith("android.permission.READ_MEDIA_") && sdk < 33) return "android.permission.READ_EXTERNAL_STORAGE";
        if (permission.equals("android.permission.READ_EXTERNAL_STORAGE") && sdk >= 33) {
            throw new IllegalArgumentException("Storage is pre-Android-13 only; request Photos, Videos or Audio");
        }
        if (permission.equals("android.permission.BLUETOOTH_CONNECT") && sdk < 31) return "android.permission.BLUETOOTH";
        if (permission.equals("android.permission.ACCESS_BACKGROUND_LOCATION") && sdk < 29) return "android.permission.ACCESS_FINE_LOCATION";
        return permission;
    }

    static String[] runtimePermissions(String permission) {
        return runtimePermissions(permission, Build.VERSION.SDK_INT);
    }

    static String[] runtimePermissions(String permission, int sdk) {
        permission = normalize(permission, sdk);
        if (permission.equals("android.permission.ACCESS_FINE_LOCATION")) {
            return new String[]{"android.permission.ACCESS_COARSE_LOCATION", permission};
        }
        if (permission.equals("android.permission.BLUETOOTH_CONNECT")) {
            return new String[]{"android.permission.BLUETOOTH_SCAN", permission};
        }
        if (permission.equals("android.permission.BLUETOOTH")) {
            // Legacy central scanning requires location as well as manifest permissions.
            return new String[]{permission, "android.permission.BLUETOOTH_ADMIN",
                "android.permission.ACCESS_COARSE_LOCATION", "android.permission.ACCESS_FINE_LOCATION"};
        }
        if (permission.equals("android.permission.READ_CALENDAR")) {
            return new String[]{permission, "android.permission.WRITE_CALENDAR"};
        }
        if (sdk >= 34 && (permission.equals("android.permission.READ_MEDIA_IMAGES")
                || permission.equals("android.permission.READ_MEDIA_VIDEO"))) {
            return new String[]{permission, "android.permission.READ_MEDIA_VISUAL_USER_SELECTED"};
        }
        return new String[]{permission};
    }

    static int aggregateStatus(String original, int sdk, int[] statuses) {
        String[] required = runtimePermissions(original, sdk);
        if (statuses.length != required.length) throw new IllegalArgumentException("Incomplete permission status set");
        if (original.equals("android.permission.ACCESS_FINE_LOCATION")) {
            if (statuses[1] == GRANTED) return GRANTED;
            if (statuses[0] == GRANTED) return LIMITED;
        }
        if (normalize(original, sdk).equals("android.permission.BLUETOOTH") && sdk < 29
                && statuses[0] == GRANTED && statuses[1] == GRANTED
                && (statuses[2] == GRANTED || statuses[3] == GRANTED)) return GRANTED;
        boolean denied = false;
        for (int status : statuses) {
            if (status == PERMANENTLY_DENIED) return PERMANENTLY_DENIED;
            if (status != GRANTED) denied = true;
        }
        return denied ? DENIED : GRANTED;
    }

    private static int individualStatus(Activity activity, String permission) {
        if (activity.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED) return GRANTED;
        boolean requested = activity.getSharedPreferences(HISTORY, Context.MODE_PRIVATE).getBoolean(permission, false);
        return requested && !activity.shouldShowRequestPermissionRationale(permission) ? PERMANENTLY_DENIED : DENIED;
    }

    public static int checkPermission(Activity activity, String original) {
        String permission = normalize(original, Build.VERSION.SDK_INT);
        if (permission.equals("android.permission.SYSTEM_ALERT_WINDOW")) return Settings.canDrawOverlays(activity) ? GRANTED : DENIED;
        if (permission.equals("android.permission.REQUEST_INSTALL_PACKAGES")) return activity.getPackageManager().canRequestPackageInstalls() ? GRANTED : DENIED;
        if (permission.equals("android.permission.ACCESS_NOTIFICATION_POLICY")) {
            NotificationManager manager = (NotificationManager) activity.getSystemService(Context.NOTIFICATION_SERVICE);
            if (manager == null) throw new IllegalStateException("Notification service unavailable");
            return manager.isNotificationPolicyAccessGranted() ? GRANTED : DENIED;
        }
        if (permission.equals("android.permission.POST_NOTIFICATIONS") && Build.VERSION.SDK_INT < 33) {
            NotificationManager manager = (NotificationManager) activity.getSystemService(Context.NOTIFICATION_SERVICE);
            if (manager == null) throw new IllegalStateException("Notification service unavailable");
            return manager.areNotificationsEnabled() ? GRANTED : DENIED;
        }
        if (permission.equals("android.permission.ACCESS_FINE_LOCATION")
                || permission.equals("android.permission.BLUETOOTH_CONNECT")
                || permission.equals("android.permission.BLUETOOTH")) {
            String[] required = runtimePermissions(original);
            int[] statuses = new int[required.length];
            for (int i = 0; i < required.length; i++) statuses[i] = individualStatus(activity, required[i]);
            return aggregateStatus(original, Build.VERSION.SDK_INT, statuses);
        }
        if (activity.checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED) {
            if (!permission.equals("android.permission.READ_CALENDAR")
                    || activity.checkSelfPermission("android.permission.WRITE_CALENDAR") == PackageManager.PERMISSION_GRANTED) return GRANTED;
            permission = "android.permission.WRITE_CALENDAR";
        }
        if (Build.VERSION.SDK_INT >= 34 && (permission.equals("android.permission.READ_MEDIA_IMAGES") || permission.equals("android.permission.READ_MEDIA_VIDEO"))
                && activity.checkSelfPermission("android.permission.READ_MEDIA_VISUAL_USER_SELECTED") == PackageManager.PERMISSION_GRANTED) return LIMITED;
        boolean requested = activity.getSharedPreferences(HISTORY, Context.MODE_PRIVATE).getBoolean(permission, false);
        return requested && !activity.shouldShowRequestPermissionRationale(permission) ? PERMANENTLY_DENIED : DENIED;
    }

    static Intent settingsIntent(Activity activity, String permission) {
        Uri app = Uri.parse("package:" + activity.getPackageName());
        switch (permission) {
            case "android.permission.SYSTEM_ALERT_WINDOW": return new Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, app);
            case "android.permission.REQUEST_INSTALL_PACKAGES": return new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, app);
            case "android.permission.ACCESS_NOTIFICATION_POLICY": return new Intent(Settings.ACTION_NOTIFICATION_POLICY_ACCESS_SETTINGS);
            case "android.permission.POST_NOTIFICATIONS":
                if (Build.VERSION.SDK_INT < 33) return new Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, activity.getPackageName());
                return null;
            default: return null;
        }
    }

    public static int requestPermission(Activity activity, String original) {
        String permission = normalize(original, Build.VERSION.SDK_INT);
        int status = checkPermission(activity, original);
        if (status == GRANTED || status == PERMANENTLY_DENIED) return status;
        return GpuiPermissionActivity.request(activity, original, settingsIntent(activity, permission));
    }

    public static String requestPermissions(Activity activity, String permissions) {
        if (permissions == null || permissions.isEmpty()) return "";
        String[] values = permissions.split("\\|", -1);
        // Validate the whole batch before prompting for any element.
        for (String value : values) normalize(value, Build.VERSION.SDK_INT);
        StringBuilder result = new StringBuilder();
        for (String value : values) {
            if (result.length() > 0) result.append('|');
            result.append(requestPermission(activity, value));
        }
        return result.toString();
    }

    public static boolean isServiceEnabled(Activity activity, int serviceType) {
        switch (serviceType) {
            case 0:
                LocationManager lm = (LocationManager) activity.getSystemService(Context.LOCATION_SERVICE);
                if (lm == null) return false;
                return lm.isProviderEnabled(LocationManager.GPS_PROVIDER) || lm.isProviderEnabled(LocationManager.NETWORK_PROVIDER);
            case 1:
                BluetoothManager bm = (BluetoothManager) activity.getSystemService(Context.BLUETOOTH_SERVICE);
                if (bm == null) return false;
                BluetoothAdapter adapter = bm.getAdapter();
                return adapter != null && adapter.isEnabled();
            default: return false;
        }
    }

    public static boolean openAppSettings(Activity activity) {
        Intent intent = new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:" + activity.getPackageName()));
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        activity.startActivity(intent);
        return true;
    }

    public static boolean shouldShowRationale(Activity activity, String original) {
        String permission = normalize(original, Build.VERSION.SDK_INT);
        if (settingsIntent(activity, permission) != null) return false;
        for (String value : runtimePermissions(permission)) {
            if (activity.shouldShowRequestPermissionRationale(value)) return true;
        }
        return false;
    }
    private GpuiPermissions() {}
}
