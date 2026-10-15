package dev.gpui.mobile;

import android.Manifest;
import android.app.Activity;
import android.app.Application;
import android.content.Context;
import android.content.pm.PackageManager;
import android.location.Location;
import android.location.LocationListener;
import android.location.LocationManager;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import java.lang.ref.WeakReference;
import java.util.HashMap;

/** Foreground-only LocationManager subscriptions. JNI callbacks never block. */
public final class GpuiLocation {
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static final HashMap<Long, Subscription> SUBSCRIPTIONS = new HashMap<>();
    private static native void nativeEvent(long id, String event);

    public static boolean isLocationEnabled(Activity activity) {
        LocationManager manager = (LocationManager) activity.getSystemService(Context.LOCATION_SERVICE);
        return manager != null && (manager.isProviderEnabled(LocationManager.GPS_PROVIDER)
            || manager.isProviderEnabled(LocationManager.NETWORK_PROVIDER));
    }
    public static void start(Activity activity, long id, int accuracy, double distance, boolean once) {
        MAIN.post(() -> {
            Subscription previous = SUBSCRIPTIONS.get(id);
            boolean paused = previous != null && previous.suspended;
            stopOnMain(id);
            if (!Double.isFinite(distance) || distance < 0 || distance > Float.MAX_VALUE) {
                nativeEvent(id, "error|configuration|Invalid distance filter");
                nativeEvent(id, "end|configuration");
                return;
            }
            Subscription subscription = new Subscription(activity, id, accuracy, distance, once);
            SUBSCRIPTIONS.put(id, subscription);
            activity.getApplication().registerActivityLifecycleCallbacks(subscription);
            if (once) MAIN.postDelayed(subscription.timeout, 30_000);
            if (paused) subscription.suspended = true;
            else subscription.resume();
        });
    }
    public static void stop(long id) { MAIN.post(() -> stopOnMain(id)); }
    private static void stopOnMain(long id) {
        Subscription subscription = SUBSCRIPTIONS.remove(id);
        if (subscription != null) {
            subscription.active = false;
            MAIN.removeCallbacks(subscription.timeout);
            subscription.suspend();
            subscription.application.unregisterActivityLifecycleCallbacks(subscription);
        }
    }
    public static String getLastKnownPosition(Activity activity) {
        LocationManager manager = (LocationManager) activity.getSystemService(Context.LOCATION_SERVICE);
        if (manager == null) return null;
        boolean fine = activity.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED;
        boolean coarse = activity.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED;
        if (!fine && !coarse) throw new SecurityException("Location permission denied");
        // SecurityException is deliberately propagated through JNI, not reported as an empty cache.
        Location best = null;
        for (String provider : manager.getProviders(true)) {
            if (!fine && !LocationManager.NETWORK_PROVIDER.equals(provider)) continue;
            Location location = manager.getLastKnownLocation(provider);
            if (location != null && (best == null || location.getElapsedRealtimeNanos() > best.getElapsedRealtimeNanos())) best = location;
        }
        return best == null ? null : serialize(best);
    }
    private static String serialize(Location p) {
        return p.getLatitude() + "|" + p.getLongitude() + "|" + (p.hasAltitude() ? p.getAltitude() : Double.NaN)
            + "|" + (p.hasAccuracy() ? p.getAccuracy() : -1)
            + "|" + (p.hasSpeed() ? p.getSpeed() : Double.NaN)
            + "|" + (p.hasSpeedAccuracy() ? p.getSpeedAccuracyMetersPerSecond() : Double.NaN)
            + "|" + (p.hasBearing() ? p.getBearing() : Double.NaN)
            + "|" + (p.hasBearingAccuracy() ? p.getBearingAccuracyDegrees() : Double.NaN) + "|" + p.getTime();
    }
    private static final class Subscription implements LocationListener, Application.ActivityLifecycleCallbacks {
        final WeakReference<Activity> activity;
        final Application application;
        final LocationManager manager;
        final long id;
        final int accuracy;
        final float distance;
        final boolean once;
        final long requestedAt = SystemClock.elapsedRealtimeNanos();
        boolean active = true;
        boolean suspended = false;
        final Runnable timeout;
        Subscription(Activity activity, long id, int accuracy, double distance, boolean once) {
            this.activity = new WeakReference<>(activity);
            this.application = activity.getApplication();
            this.manager = (LocationManager) activity.getSystemService(Context.LOCATION_SERVICE);
            this.id = id; this.accuracy = accuracy; this.distance = (float) distance; this.once = once;
            timeout = () -> { emit("error|timeout|No fresh location in 30 seconds"); finish("timeout"); };
        }
        void emit(String event) { if (active) nativeEvent(id, event); }
        void finish(String reason) { emit("end|" + reason); stopOnMain(id); }
        void suspend() {
            suspended = true;
            if (manager != null) {
                try { manager.removeUpdates(this); }
                catch (SecurityException error) { emit("error|permission_denied|" + error.getMessage()); }
            }
        }
        void resume() {
            if (!active) return;
            Activity owner = activity.get();
            if (owner == null) { finish("owner_destroyed"); return; }
            suspend();
            suspended = false;
            boolean fine = owner.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED;
            boolean coarse = owner.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED;
            emit("permission|" + (fine ? "precise" : coarse ? "approximate" : "denied"));
            if (!fine && !coarse) return;
            if (manager == null) { emit("error|unavailable|LocationManager unavailable"); return; }
            try {
                String provider = null;
                if (accuracy >= 3 && fine && manager.isProviderEnabled(LocationManager.GPS_PROVIDER)) provider = LocationManager.GPS_PROVIDER;
                else if (manager.isProviderEnabled(LocationManager.NETWORK_PROVIDER)) provider = LocationManager.NETWORK_PROVIDER;
                else if (fine && manager.isProviderEnabled(LocationManager.GPS_PROVIDER)) provider = LocationManager.GPS_PROVIDER;
                if (provider == null) { emit("error|disabled|No enabled location provider; refresh after enabling"); return; }
                manager.requestLocationUpdates(provider, 1000, distance, this, Looper.getMainLooper());
            } catch (SecurityException error) {
                emit("permission|denied"); emit("error|permission_denied|" + error.getMessage());
            } catch (IllegalArgumentException error) { emit("error|native|" + error.getMessage()); }
        }
        @Override public void onLocationChanged(Location location) {
            if (!active || suspended || (once && location.getElapsedRealtimeNanos() < requestedAt)) return;
            emit("position|" + serialize(location));
            if (once) finish("completed");
        }
        @Override public void onProviderDisabled(String provider) { emit("error|disabled|" + provider); }
        @Override public void onProviderEnabled(String provider) { if (!suspended) resume(); }
        @Override public void onStatusChanged(String provider, int status, Bundle extras) {}
        @Override public void onActivityResumed(Activity a) { if (activity.get() == a) resume(); }
        @Override public void onActivityPaused(Activity a) { if (activity.get() == a) suspend(); }
        @Override public void onActivityDestroyed(Activity a) { if (activity.get() == a) finish("owner_destroyed"); }
        @Override public void onActivityCreated(Activity a, Bundle state) {}
        @Override public void onActivityStarted(Activity a) {}
        @Override public void onActivityStopped(Activity a) {}
        @Override public void onActivitySaveInstanceState(Activity a, Bundle state) {}
    }
    private GpuiLocation() {}
}
