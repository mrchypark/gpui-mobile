package dev.gpui.mobile;

import android.app.Activity;
import android.os.Looper;
import android.support.v4.media.session.MediaControllerCompat;

/** Invoke from a real attached worker/native thread with the Activity resumed.
 * Do not join this worker from the main thread. Uses the real Android session.
 */
public final class GpuiMediaSessionNativeTest {
    public static void run(Activity activity) throws Exception {
        if (Looper.myLooper() != null) throw new AssertionError("Requires thread without Looper");
        for (int i = 0; i < 2; i++) {
            GpuiMediaSession.init(activity);
            Object token = GpuiMediaSession.getSessionToken();
            if (token == null) throw new AssertionError("init did not complete synchronously");
            GpuiMediaSession.init(activity);
            if (!token.equals(GpuiMediaSession.getSessionToken())) throw new AssertionError("duplicate session");
            GpuiMediaSession.setMetadata("native regression", "GPUI", 12000);
            MediaControllerCompat controller = new MediaControllerCompat(activity, GpuiMediaSession.getSessionToken());
            if (!"native regression".equals(controller.getMetadata().getDescription().getTitle().toString()))
                throw new AssertionError("metadata mutation not completed");
            GpuiMediaSession.release();
            GpuiMediaSession.release();
            if (GpuiMediaSession.getSessionToken() != null) throw new AssertionError("release retained session");
        }
        try {
            GpuiMediaSession.setMetadata("after release", "", 0);
            throw new AssertionError("uninitialized mutation reported success");
        } catch (IllegalStateException expected) { }
    }
}
