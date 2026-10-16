package dev.gpui.mobile;

import android.app.Activity;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.Context;
import android.os.Build;
import android.os.Bundle;

import androidx.core.app.NotificationCompat;
import androidx.core.app.NotificationManagerCompat;

/**
 * Java helper for local notifications on Android.
 *
 * <p>All public methods are {@code static} and called from Rust via JNI.</p>
 */
public final class GpuiNotifications {

    private static final String DEFAULT_CHANNEL_ID = "default";
    private static final String DEFAULT_CHANNEL_NAME = "Default";
    private static final String DEFAULT_CHANNEL_DESC = "Default notification channel";

    /**
     * Initialize the notification system by creating the default notification channel.
     *
     * @param activity The current Activity.
     */
    public static void initialize(Activity activity) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationManager nm = (NotificationManager) activity.getSystemService(Context.NOTIFICATION_SERVICE);
            if (nm == null) throw new IllegalStateException("Notification service unavailable");
            if (nm != null) {
                NotificationChannel channel = new NotificationChannel(
                        DEFAULT_CHANNEL_ID,
                        DEFAULT_CHANNEL_NAME,
                        NotificationManager.IMPORTANCE_DEFAULT
                );
                channel.setDescription(DEFAULT_CHANNEL_DESC);
                nm.createNotificationChannel(channel);
            }
        }
    }

    /**
     * Show a notification.
     *
     * @param activity    The current Activity.
     * @param id          Notification ID (for updating/canceling).
     * @param title       Notification title.
     * @param body        Notification body text.
     * @param channelId   Notification channel ID.
     * @param channelName Notification channel name.
     * @param channelDesc Notification channel description.
     * @param importance  Importance level (0=min, 1=low, 2=default, 3=high, 4=max).
     * @param payload     Optional payload string (may be empty).
     */
    public static void show(Activity activity, int id, String title, String body,
                            String channelId, String channelName, String channelDesc,
                            int importance, String payload) {
        NotificationManager nm = (NotificationManager) activity.getSystemService(Context.NOTIFICATION_SERVICE);
        if (nm == null) {
            throw new IllegalStateException("Notification service unavailable");
        }
        if (!NotificationManagerCompat.from(activity).areNotificationsEnabled()) {
            throw new IllegalStateException("Notifications are disabled for this app");
        }

        // Create or update the notification channel (API 26+)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            int androidImportance = mapImportance(importance);
            NotificationChannel channel = new NotificationChannel(channelId, channelName, androidImportance);
            channel.setDescription(channelDesc);
            nm.createNotificationChannel(channel);
            if (nm.getNotificationChannel(channelId).getImportance() == NotificationManager.IMPORTANCE_NONE) {
                throw new IllegalStateException("Notification channel is disabled");
            }
        }

        // Build the notification
        NotificationCompat.Builder builder = new NotificationCompat.Builder(activity, channelId)
                .setSmallIcon(android.R.drawable.ic_dialog_info)
                .setContentTitle(title)
                .setContentText(body)
                .setAutoCancel(true)
                .setPriority(mapPriority(importance));

        if (payload != null && !payload.isEmpty()) {
            Bundle extras = new Bundle();
            extras.putString("payload", payload);
            builder.addExtras(extras);
        }

        nm.notify(id, builder.build());
    }

    /**
     * Cancel a specific notification by ID.
     *
     * @param activity The current Activity.
     * @param id       The notification ID to cancel.
     */
    public static void cancel(Activity activity, int id) {
        NotificationManager nm = (NotificationManager) activity.getSystemService(Context.NOTIFICATION_SERVICE);
        if (nm == null) throw new IllegalStateException("Notification service unavailable");
        if (nm != null) {
            nm.cancel(id);
        }
    }

    /**
     * Cancel all notifications.
     *
     * @param activity The current Activity.
     */
    public static void cancelAll(Activity activity) {
        NotificationManager nm = (NotificationManager) activity.getSystemService(Context.NOTIFICATION_SERVICE);
        if (nm == null) throw new IllegalStateException("Notification service unavailable");
        if (nm != null) {
            nm.cancelAll();
        }
    }

    /**
     * Map our importance level (0-4) to Android NotificationManager.IMPORTANCE_* constants.
     */
    private static int mapImportance(int importance) {
        switch (importance) {
            case 0: return NotificationManager.IMPORTANCE_MIN;      // 1
            case 1: return NotificationManager.IMPORTANCE_LOW;      // 2
            case 2: return NotificationManager.IMPORTANCE_DEFAULT;  // 3
            case 3: return NotificationManager.IMPORTANCE_HIGH;     // 4
            case 4: return NotificationManager.IMPORTANCE_MAX;      // 5
            default: return NotificationManager.IMPORTANCE_DEFAULT; // 3
        }
    }

    /**
     * Map our importance level (0-4) to NotificationCompat priority constants.
     * Used for pre-API-26 devices.
     */
    private static int mapPriority(int importance) {
        switch (importance) {
            case 0: return NotificationCompat.PRIORITY_MIN;
            case 1: return NotificationCompat.PRIORITY_LOW;
            case 2: return NotificationCompat.PRIORITY_DEFAULT;
            case 3: return NotificationCompat.PRIORITY_HIGH;
            case 4: return NotificationCompat.PRIORITY_MAX;
            default: return NotificationCompat.PRIORITY_DEFAULT;
        }
    }

    private static native void nativeWake();

    private static android.app.PendingIntent responseIntent(Activity activity, String tag, String action) {
        android.net.Uri.Builder identity = new android.net.Uri.Builder().scheme("gpui-notification")
                .authority(activity.getPackageName()).appendPath(tag).appendPath(action == null ? "body" : "action");
        if (action != null) identity.appendPath(action);
        android.content.Intent intent = new android.content.Intent(activity, GpuiNotificationActivity.class)
                .setData(identity.build()).putExtra("tag", tag);
        if (action != null) intent.putExtra("action", action);
        return android.app.PendingIntent.getActivity(activity, 0, intent,
                android.app.PendingIntent.FLAG_UPDATE_CURRENT | android.app.PendingIntent.FLAG_IMMUTABLE);
    }

    public static void showSystem(Activity activity, String tag, String title, String body,
                                  String[] ids, String[] labels) {
        if (tag == null || ids.length != labels.length) throw new IllegalArgumentException("Invalid notification identity/actions");
        java.util.HashSet<String> unique = new java.util.HashSet<>();
        for (String id : ids) if (id == null || !unique.add(id)) throw new IllegalArgumentException("Duplicate/null action id");
        initialize(activity); // Channel creation only; never requests authorization.
        if (!NotificationManagerCompat.from(activity).areNotificationsEnabled())
            throw new IllegalStateException("Notifications are disabled for this app");
        NotificationManager manager = (NotificationManager) activity.getSystemService(Context.NOTIFICATION_SERVICE);
        if (manager == null) throw new IllegalStateException("Notification service unavailable");
        if (Build.VERSION.SDK_INT >= 26 && manager.getNotificationChannel(DEFAULT_CHANNEL_ID).getImportance() == NotificationManager.IMPORTANCE_NONE)
            throw new IllegalStateException("Notification channel is disabled");
        NotificationCompat.Builder builder = new NotificationCompat.Builder(activity, DEFAULT_CHANNEL_ID)
                .setSmallIcon(android.R.drawable.ic_dialog_info).setContentTitle(title).setContentText(body)
                .setAutoCancel(true).setContentIntent(responseIntent(activity, tag, null));
        for (int i = 0; i < ids.length; i++) builder.addAction(0, labels[i], responseIntent(activity, tag, ids[i]));
        manager.notify(tag, 0, builder.build());
    }

    public static void cancelSystem(Activity activity, String tag) {
        NotificationManager manager = (NotificationManager) activity.getSystemService(Context.NOTIFICATION_SERVICE);
        if (manager == null) throw new IllegalStateException("Notification service unavailable");
        manager.cancel(tag, 0);
    }

    static void recordResponse(Activity activity, String tag, String action) {
        synchronized (GpuiNotifications.class) {
            android.content.SharedPreferences prefs = activity.getSharedPreferences("gpui_notification_responses", Context.MODE_PRIVATE);
            try {
                org.json.JSONArray queue = new org.json.JSONArray(prefs.getString("queue", "[]"));
                queue.put(new org.json.JSONArray().put(tag).put(action == null ? org.json.JSONObject.NULL : action));
                if (!prefs.edit().putString("queue", queue.toString()).commit()) throw new IllegalStateException("Cannot persist notification response");
            } catch (org.json.JSONException error) { throw new IllegalStateException("Invalid notification response queue", error); }
        }
        try { nativeWake(); } catch (UnsatisfiedLinkError unavailable) { /* Startup poll drains durable queue. */ }
    }

    public static synchronized String[] drainResponses(Activity activity) {
        android.content.SharedPreferences prefs = activity.getSharedPreferences("gpui_notification_responses", Context.MODE_PRIVATE);
        try {
            org.json.JSONArray queue = new org.json.JSONArray(prefs.getString("queue", "[]"));
            String[] result = new String[queue.length() * 3];
            for (int i = 0; i < queue.length(); i++) {
                org.json.JSONArray response = queue.getJSONArray(i);
                result[i * 3] = response.getString(0);
                result[i * 3 + 1] = response.isNull(1) ? "0" : "1";
                result[i * 3 + 2] = response.isNull(1) ? "" : response.getString(1);
            }
            if (!prefs.edit().remove("queue").commit()) throw new IllegalStateException("Cannot drain notification responses");
            return result;
        } catch (org.json.JSONException error) { throw new IllegalStateException("Invalid notification response queue", error); }
    }
    // Prevent instantiation.
    private GpuiNotifications() {}
}
