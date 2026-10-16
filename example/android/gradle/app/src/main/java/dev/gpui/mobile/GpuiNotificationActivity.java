package dev.gpui.mobile;

/** Non-exported notification activation endpoint. Native callbacks only enqueue events. */
public final class GpuiNotificationActivity extends android.app.Activity {
    @Override public void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        if (state == null) {
            android.content.Intent intent = getIntent();
            String tag = intent.getStringExtra("tag");
            if (tag != null) {
                try {
                    GpuiNotifications.recordResponse(this, tag, intent.hasExtra("action") ? intent.getStringExtra("action") : null);
                    GpuiNotifications.cancelSystem(this, tag);
                } catch (RuntimeException error) { android.util.Log.e("GpuiNotifications", "Notification response failed", error); }
                android.content.Intent launch = getPackageManager().getLaunchIntentForPackage(getPackageName());
                if (launch != null) startActivity(launch.addFlags(android.content.Intent.FLAG_ACTIVITY_CLEAR_TOP | android.content.Intent.FLAG_ACTIVITY_SINGLE_TOP));
            }
        }
        finish();
    }
}
