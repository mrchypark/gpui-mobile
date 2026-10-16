package dev.gpui.mobile;

import android.app.Activity;
import android.os.Looper;

/** Run in the real Android host on its UI thread; no fake ReviewManager is used. */
public final class GpuiInAppReviewNativeTest {
    public static void rejectUiThread(Activity activity) {
        if (Looper.myLooper() != Looper.getMainLooper()) throw new IllegalStateException("UI thread required");
        String error = GpuiInAppReview.requestReview(activity);
        if (error == null || !error.contains("UI thread")) throw new AssertionError(error);
    }
}
