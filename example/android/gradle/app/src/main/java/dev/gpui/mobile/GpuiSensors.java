package dev.gpui.mobile;

import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.hardware.Sensor;
import android.hardware.SensorEvent;
import android.hardware.SensorEventListener;
import android.hardware.SensorManager;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.util.SparseArray;
import java.lang.ref.WeakReference;

/** Lazy native sampling; no sample is represented by null, never invented zeroes. */
public final class GpuiSensors implements SensorEventListener, Application.ActivityLifecycleCallbacks {
    private static GpuiSensors instance;
    private static final java.util.WeakHashMap<Activity, Boolean> hostResumed = new java.util.WeakHashMap<>();

    /** Host calls true after super.onResume; false BEFORE super.onPause/onDestroy.
     * Unknown hosts fail closed. This state exists before lazy sensor creation.
     */
    public static synchronized void setHostResumed(Activity activity, boolean resumed) {
        hostResumed.put(activity, resumed);
        if (instance != null && instance.owner.get() == activity) {
            synchronized (instance) {
                instance.paused = !resumed;
                if (!resumed) instance.stop();
            }
        }
    }
    private final SensorManager manager;
    private final WeakReference<Activity> owner;
    private final SparseArray<double[]> readings = new SparseArray<>();
    private final SparseArray<Sensor> listening = new SparseArray<>();
    private final Handler handler = new Handler(Looper.getMainLooper());
    private boolean paused;
    private boolean retired;
    private long epoch;

    private GpuiSensors(Activity activity) {
        paused = !Boolean.TRUE.equals(hostResumed.get(activity));
        owner = new WeakReference<>(activity);
        manager = (SensorManager) activity.getApplicationContext().getSystemService(Context.SENSOR_SERVICE);
        activity.getApplication().registerActivityLifecycleCallbacks(this);
    }
    private static synchronized GpuiSensors get(Activity activity) {
        if (instance == null || instance.owner.get() != activity) {
            if (instance != null) {
                instance.retire();
                activity.getApplication().unregisterActivityLifecycleCallbacks(instance);
            }
            instance = new GpuiSensors(activity);
        }
        if (activity.isFinishing() || activity.isDestroyed()) {
            instance.retire();
            activity.getApplication().unregisterActivityLifecycleCallbacks(instance);
        }
        return instance;
    }
    public static int available(Activity activity) {
        SensorManager manager = (SensorManager) activity.getSystemService(Context.SENSOR_SERVICE);
        if (manager == null) return 0;
        int bits = 0;
        int[] types = { Sensor.TYPE_ACCELEROMETER, Sensor.TYPE_GYROSCOPE,
            Sensor.TYPE_MAGNETIC_FIELD, Sensor.TYPE_PRESSURE };
        for (int i = 0; i < types.length; i++) {
            if (manager.getDefaultSensor(types[i]) != null) bits |= 1 << i;
        }
        return bits;
    }
    public static synchronized double[] read(Activity activity, int type) {
        if (activity.isFinishing() || activity.isDestroyed()) return null;
        return get(activity).sample(type);
    }
    private synchronized double[] sample(int type) {
        if (retired || paused || manager == null) return null;
        if (listening.get(type) == null) {
            Sensor sensor = manager.getDefaultSensor(type);
            if (sensor == null) return null;
            if (listening.size() == 0) epoch = SystemClock.elapsedRealtimeNanos();
            try {
                if (!manager.registerListener(this, sensor, SensorManager.SENSOR_DELAY_UI, handler)) return null;
                listening.put(type, sensor);
            } catch (RuntimeException error) {
                android.util.Log.w("GpuiSensors", "Sensor registration failed", error);
                return null;
            }
        }
        double[] reading = readings.get(type);
        return reading == null ? null : reading.clone();
    }
    @Override public synchronized void onSensorChanged(SensorEvent event) {
        int type = event.sensor.getType();
        if (paused || listening.get(type) == null || event.timestamp < epoch) return;
        double[] value = reading(type, event.values);
        if (value == null) readings.remove(type);
        else readings.put(type, value);
    }
    static double[] reading(int type, float[] values) {
        int count = type == Sensor.TYPE_PRESSURE ? 1 : 3;
        if (values == null || values.length < count) return null;
        double[] value = new double[3];
        for (int i = 0; i < count; i++) {
            if (Float.isNaN(values[i]) || Float.isInfinite(values[i])) return null;
            value[i] = values[i];
        }
        // Android already supplies m/s², rad/s, µT and hPa.
        if (type == Sensor.TYPE_PRESSURE && value[0] <= 0) return null;
        return value;
    }
    @Override public void onAccuracyChanged(Sensor sensor, int accuracy) {}
    private synchronized void stop() {
        if (manager != null) manager.unregisterListener(this);
        listening.clear();
        readings.clear();
        epoch = SystemClock.elapsedRealtimeNanos();
    }
    private synchronized void retire() {
        retired = true;
        stop();
    }
    @Override public void onActivityPaused(Activity activity) {
        if (owner.get() == activity) setHostResumed(activity, false);
    }
    @Override public void onActivityResumed(Activity activity) {
        // Host hook is authoritative; callbacks alone cannot seed first use.
    }
    @Override public void onActivityDestroyed(Activity activity) {
        if (owner.get() == activity) {
            synchronized (GpuiSensors.class) {
                retire();
                activity.getApplication().unregisterActivityLifecycleCallbacks(this);
                if (instance == this) instance = null;
                hostResumed.remove(activity);
            }
        }
    }
    @Override public void onActivityCreated(Activity a, Bundle b) {}
    @Override public void onActivityStarted(Activity a) {}
    @Override public void onActivityStopped(Activity a) {}
    @Override public void onActivitySaveInstanceState(Activity a, Bundle b) {}
}
