package dev.gpui.mobile;

import android.app.Activity;
import android.hardware.Sensor;
import android.os.Looper;
import android.util.SparseArray;
import java.lang.reflect.Field;

/** Real SensorManager regression. Run on main with a fresh resumed Activity,
 * before its first sensor read; a device accelerometer is required.
 */
public final class GpuiSensorsLifecycleNativeTest {
    private static Object instance() throws Exception {
        Field field = GpuiSensors.class.getDeclaredField("instance");
        field.setAccessible(true);
        return field.get(null);
    }
    private static int registrations() throws Exception {
        Field field = GpuiSensors.class.getDeclaredField("listening");
        field.setAccessible(true);
        return ((SparseArray<?>) field.get(instance())).size();
    }
    public static void run(Activity activity) throws Exception {
        if (Looper.myLooper() != Looper.getMainLooper()) throw new AssertionError("Requires main thread");
        if (instance() != null) throw new AssertionError("Requires fresh sensor singleton");
        if ((GpuiSensors.available(activity) & 1) == 0) throw new AssertionError("Requires accelerometer");
        try {
            // Pause BEFORE first use: no observer exists to receive the earlier event.
            GpuiSensors.setHostResumed(activity, false);
            if (GpuiSensors.read(activity, Sensor.TYPE_ACCELEROMETER) != null || registrations() != 0)
                throw new AssertionError("first read while paused registered listener");
            GpuiSensors.setHostResumed(activity, true);
            GpuiSensors.read(activity, Sensor.TYPE_ACCELEROMETER);
            if (registrations() != 1) throw new AssertionError("resume did not permit registration");
            GpuiSensors.read(activity, Sensor.TYPE_ACCELEROMETER);
            if (registrations() != 1) throw new AssertionError("poll duplicated listener");
            GpuiSensors.setHostResumed(activity, false);
            if (GpuiSensors.read(activity, Sensor.TYPE_ACCELEROMETER) != null || registrations() != 0)
                throw new AssertionError("re-pause retained listeners/readings");
        } finally {
            GpuiSensors.setHostResumed(activity, false);
            ((GpuiSensors) instance()).onActivityDestroyed(activity);
        }
    }
}
