package dev.gpui.mobile;

import android.app.Activity;
import android.hardware.camera2.CameraManager;
import android.content.Context;
import android.os.Looper;
import android.util.Size;
import java.lang.reflect.Method;

/** Device-host self-check; run on a worker with CAMERA permission and two real cameras. */
public final class GpuiCameraNativeTest {
    public static void run(Activity activity) throws Exception {
        if (Looper.myLooper() == Looper.getMainLooper()) throw new IllegalStateException("Worker thread required");
        Method choose = GpuiCamera.class.getDeclaredMethod("chooseSize", Size[].class, int.class);
        choose.setAccessible(true);
        Size largest = (Size) choose.invoke(null, new Size[] {new Size(640, 480), new Size(3840, 2160)}, 5);
        check(largest.equals(new Size(3840, 2160)));
        CameraManager manager = (CameraManager) activity.getSystemService(Context.CAMERA_SERVICE);
        String[] ids = manager.getCameraIdList();
        if (ids.length < 2) throw new IllegalStateException("Two cameras required; test not run");
        int handle = GpuiCamera.createCamera(activity, ids[0], 2, false);
        check(handle > 0);
        try {
            check(GpuiCamera.setCamera(handle, "gpui.nonexistent.camera") != null);
            check(GpuiCamera.setCamera(handle, ids[1]) == null);
            check(GpuiCamera.setCamera(handle, ids[1]) == null);
            check(GpuiCamera.setCamera(handle, ids[0]) == null);
        } finally { GpuiCamera.dispose(handle); }
        check(GpuiCamera.setCamera(handle, ids[1]) != null);
        // Reopening after dispose checks that the real device resource was released.
        int reopened = GpuiCamera.createCamera(activity, ids[0], 2, false);
        check(reopened > 0);
        GpuiCamera.dispose(reopened);
    }
    private static void check(boolean value) { if (!value) throw new AssertionError(); }
}
