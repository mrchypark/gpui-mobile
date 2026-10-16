package dev.gpui.mobile;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;

public class GpuiDeepLink {
    private static boolean captured;
    private static String initialLink;

    /** Call from Activity.onCreate before native startup and before any setIntent replacement. */
    public static synchronized void captureInitialLink(Activity activity) {
        if (captured) return;
        initialLink = readLink(activity);
        captured = true;
    }

    public static synchronized String getInitialLink(Activity activity) {
        if (!captured) throw new IllegalStateException("Host has not captured the initial launch URL");
        return initialLink;
    }

    private static String readLink(Activity activity) {
        if (activity == null) return null;
        Intent intent = activity.getIntent();
        if (intent == null) return null;
        String action = intent.getAction();
        Uri data = intent.getData();
        if (data != null && (Intent.ACTION_VIEW.equals(action) || Intent.ACTION_MAIN.equals(action))) {
            return data.toString();
        }
        return null;
    }
}
