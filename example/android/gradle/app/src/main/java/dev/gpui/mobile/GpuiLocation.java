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

/** LocationManager subscriptions. JNI callbacks never block. */
public final class GpuiLocation {
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static final HashMap<Long, Subscription> SUBSCRIPTIONS = new HashMap<>();
    private static native void nativeEvent(long id, String event);

    static final class HostState {
        private static final java.util.WeakHashMap<Object, Boolean> RESUMED = new java.util.WeakHashMap<>();
        static boolean resumed(Object owner) { return Boolean.TRUE.equals(RESUMED.get(owner)); }
        static void set(Object owner, boolean resumed) { RESUMED.put(owner, resumed); }
    }

    /** Host calls on main in onResume/onPause, before submitting location work. */
    public static void setHostResumed(Activity activity, boolean resumed) {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            throw new IllegalStateException("Location host lifecycle must run on main");
        }
        boolean changed = HostState.resumed(activity) != resumed;
        HostState.set(activity, resumed);
        if (!changed) return;
        for (Subscription subscription : new java.util.ArrayList<>(SUBSCRIPTIONS.values())) {
            if (subscription.activity.get() == activity) {
                subscription.lockedReceipt.hostState(resumed);
                if (!resumed) subscription.logPowerState("pause");
                if (resumed) subscription.resume();
                else if (!subscription.background) subscription.suspend();
            }
        }
    }

    /** Diagnostic guard only; it does not change subscriptions or delivery. */
    static final class LockedReceipt {
        private boolean paused;
        private boolean logged;
        private long epoch;
        void hostState(boolean resumed) {
            if (!resumed && !paused) { epoch++; logged = false; }
            paused = !resumed;
        }
        boolean pending() { return paused && !logged; }
        long epoch() { return epoch; }
        boolean record(boolean resumed, boolean interactive, boolean locked) {
            if (!pending() || resumed || interactive || !locked) return false;
            logged = true;
            return true;
        }
    }

    static final class Fix {
        static boolean valid(double latitude, double longitude, boolean hasAccuracy, float accuracy, long timestamp) {
            return hasAccuracy && Float.isFinite(accuracy) && accuracy >= 0
                && Double.isFinite(latitude) && Math.abs(latitude) <= 90
                && Double.isFinite(longitude) && Math.abs(longitude) <= 180 && timestamp >= 0;
        }
    }

    static final class RegistrationState {
        private String provider;
        boolean preserve(boolean permitted, String selected) {
            return permitted && selected != null && selected.equals(provider);
        }
        void registered(String selected) { provider = selected; }
        void removed() { provider = null; }
    }

    static final class ProviderPolicy {
        static String[] candidates(boolean fused, boolean fine, boolean high) {
            String first = high && fine ? "gps" : "network";
            String second = high && fine ? "network" : "gps";
            if (!fine) return fused ? new String[]{"fused", "network"} : new String[]{"network"};
            return fused ? new String[]{"fused", first, second} : new String[]{first, second};
        }
        static boolean deliver(boolean active, boolean suspended, long fix, long requested, long last) {
            return active && !suspended && fix >= requested && fix > last;
        }
    }

    static final class CallbackCounts {
        long received, accepted, inactive, paused, stale, duplicate, outOfOrder, invalid;
        String record(boolean active, boolean suspended, long fix, long requested, long last, boolean valid) {
            received++;
            if (!active) { inactive++; return "inactive"; }
            if (suspended) { paused++; return "paused"; }
            if (fix < requested) { stale++; return "pre_request"; }
            if (fix == last) { duplicate++; return "duplicate"; }
            if (fix < last) { outOfOrder++; return "out_of_order"; }
            if (!valid) { invalid++; return "invalid_fix"; }
            accepted++; return "accepted";
        }
        String summary() {
            return " received=" + received + " accepted=" + accepted + " inactive=" + inactive
                + " paused=" + paused + " pre_request=" + stale + " duplicate=" + duplicate
                + " out_of_order=" + outOfOrder + " invalid_fix=" + invalid;
        }
    }
    static final class ProbeReceipt {
        long timestamp, fix, receipt, highTimestamp = -1;
        boolean resumed, unique;
        void record(long timestamp, long fix, long receipt, boolean resumed) {
            unique = timestamp > highTimestamp;
            highTimestamp = Math.max(highTimestamp, timestamp);
            this.timestamp = timestamp; this.fix = fix; this.receipt = receipt; this.resumed = resumed;
        }
        boolean matches(long timestamp, long boundary, boolean resumed) {
            return unique && boundary > 0 && this.timestamp == timestamp && this.resumed == resumed
                && fix > boundary && receipt > boundary;
        }
    }
    static final class ProbeMove {
        boolean attempted;
        boolean move(boolean enabled, boolean owned, boolean resumed, java.util.function.BooleanSupplier action) {
            if (!enabled || !owned || !resumed || attempted) return false;
            attempted = true;
            return action.getAsBoolean();
        }
    }

    public static boolean isLocationEnabled(Activity activity) {
        LocationManager manager = (LocationManager) activity.getSystemService(Context.LOCATION_SERVICE);
        return manager != null && (manager.isProviderEnabled(LocationManager.GPS_PROVIDER)
            || manager.isProviderEnabled(LocationManager.NETWORK_PROVIDER));
    }
    public static void start(Activity activity, long id, int accuracy, double distance, boolean once, long timeoutMs, boolean background) {
        MAIN.post(() -> {
            Subscription existing = SUBSCRIPTIONS.get(id);
            if (existing != null && existing.background && background) { existing.resume(); return; }
            if (activity.isDestroyed() || activity.isFinishing()) {
                stopOnMain(id);
                nativeEvent(id, "end|owner_destroyed");
                return;
            }
            stopOnMain(id);
            if (GpuiGmsLocation.cleanupPending()) {
                nativeEvent(id, "error|cleanup_pending|Previous GMS listener cleanup is pending; retry after cleanup");
                nativeEvent(id, "end|cleanup_pending");
                return;
            }
            if (!Double.isFinite(distance) || distance < 0 || distance > Float.MAX_VALUE
                || (once && (timeoutMs < 1 || timeoutMs > 86_400_000))) {
                nativeEvent(id, "error|configuration|Invalid distance filter or timeout");
                nativeEvent(id, "end|configuration");
                return;
            }
            if (background && !HostState.resumed(activity) && !GpuiLocationService.isReady()) {
                nativeEvent(id, "error|background_start|Start background tracking from a resumed Activity");
                nativeEvent(id, "end|background_start");
                return;
            }
            Subscription subscription = new Subscription(activity, id, accuracy, distance, once, background);
            SUBSCRIPTIONS.put(id, subscription);
            activity.getApplication().registerActivityLifecycleCallbacks(subscription);
            if (once) MAIN.postDelayed(subscription.timeout, timeoutMs);
            if (background) {
                boolean permission = activity.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED
                    || activity.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED;
                if (!permission) {
                    subscription.emit("error|permission_denied|Grant location permission before starting background tracking");
                    subscription.finish("permission_denied");
                    return;
                }
                try { GpuiLocationService.acquire(subscription.application, id); }
                catch (RuntimeException error) {
                    subscription.emit("error|background_start|" + error.getClass().getSimpleName());
                    subscription.finish("background_start");
                }
            } else if (!HostState.resumed(activity)) subscription.suspended = true;
            else subscription.resume();
        });
    }
    public static void stop(long id) { MAIN.post(() -> stopOnMain(id)); }
    // Called only by the DEBUG host probe. Snapshot is read on main, never by
    // racing the native worker against the subscription/retirement maps.
    static final class ProbeBackend {
        static int flags(boolean active, boolean suspended, boolean gmsRegistered, boolean platformRegistered) {
            if (!active || suspended || gmsRegistered == platformRegistered) return 0;
            return 2 | (gmsRegistered ? 32 : 128);
        }
        static boolean phase(int state, boolean resumed) {
            int required = resumed ? 7 : 11;
            return state >= 0 && (state & required) == required
                && ((state & 32) != 0) != ((state & 128) != 0)
                && (resumed || (state & 4) == 0);
        }
    }
    static int probeState(long id) {
        java.util.concurrent.FutureTask<Integer> query = new java.util.concurrent.FutureTask<>(() -> {
            Subscription s = SUBSCRIPTIONS.get(id);
            int flags = GpuiLocationService.isReady() ? 8 : 0;
            if (!GpuiGmsLocation.RETIRED.isEmpty()) flags |= 16;
            if (s != null) {
                flags |= 1;
                if (HostState.resumed(s.activity.get())) flags |= 4;
                flags |= ProbeBackend.flags(s.active, s.suspended,
                    s.gms != null && s.gms.registered(), s.registration.provider != null);
                if (s.gms == null && s.suspended && s.registration.provider == null) flags |= 64;
            }
            return flags;
        });
        if (Looper.myLooper() == Looper.getMainLooper()) query.run(); else MAIN.post(query);
        try { return query.get(2, java.util.concurrent.TimeUnit.SECONDS); }
        catch (Exception error) { return -1; }
    }
    static long probePhase(long id, long timestamp, long boundary, boolean resumed) {
        java.util.concurrent.FutureTask<Long> query = new java.util.concurrent.FutureTask<>(() -> {
            int state = probeState(id);
            if (!ProbeBackend.phase(state, resumed)) return 0L;
            if (boundary == 0) return SystemClock.elapsedRealtimeNanos();
            Subscription s = SUBSCRIPTIONS.get(id);
            return s != null && s.probeReceipt.matches(timestamp, boundary, resumed) ? 1L : 0L;
        });
        if (Looper.myLooper() == Looper.getMainLooper()) query.run(); else MAIN.post(query);
        try { return query.get(2, java.util.concurrent.TimeUnit.SECONDS); }
        catch (Exception error) { return -1; }
    }
    static boolean probeMoveToBackground(long id, java.util.function.BooleanSupplier enabled) {
        java.util.concurrent.FutureTask<Boolean> query = new java.util.concurrent.FutureTask<>(() -> {
            Subscription s = SUBSCRIPTIONS.get(id);
            if (s == null) return false;
            Activity activity = s.activity.get();
            boolean owned = s.active && !s.suspended && activity != null
                && !activity.isFinishing() && !activity.isDestroyed()
                && ProbeBackend.phase(probeState(id), true);
            return s.probeMove.move(enabled.getAsBoolean(), owned, HostState.resumed(activity),
                () -> activity.moveTaskToBack(true));
        });
        if (Looper.myLooper() == Looper.getMainLooper()) query.run(); else MAIN.post(query);
        try { return query.get(2, java.util.concurrent.TimeUnit.SECONDS); }
        catch (Exception error) { query.cancel(false); return false; }
    }
    private static void stopOnMain(long id) {
        Subscription subscription = SUBSCRIPTIONS.remove(id);
        if (subscription != null) {
            subscription.active = false;
            if (subscription.gms != null) GpuiGmsLocation.retire(subscription.gms);
            MAIN.removeCallbacks(subscription.timeout);
            subscription.suspend();
            subscription.application.unregisterActivityLifecycleCallbacks(subscription);
            if (subscription.background) GpuiLocationService.release(subscription.application, id);
        }
    }
    static void backgroundReady() {
        for (Subscription s : new java.util.ArrayList<>(SUBSCRIPTIONS.values())) {
            if (s.background) s.resume();
        }
    }
    static void backgroundStopped(String reason) {
        for (Subscription s : new java.util.ArrayList<>(SUBSCRIPTIONS.values())) {
            if (s.background) { s.emit("error|background_service|" + reason); s.finish(reason); }
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
            if (location != null && Fix.valid(location.getLatitude(), location.getLongitude(),
                location.hasAccuracy(), location.getAccuracy(), location.getTime())
                && (best == null || location.getElapsedRealtimeNanos() > best.getElapsedRealtimeNanos())) best = location;
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
        final boolean background;
        final long requestedAt = SystemClock.elapsedRealtimeNanos();
        boolean active = true;
        boolean suspended = false;
        boolean loggedFirstFix = false;
        long registeredAt;
        long lastFix = -1;
        long lastReceipt;
        final CallbackCounts callbackCounts = new CallbackCounts();
        final ProbeReceipt probeReceipt = new ProbeReceipt();
        final ProbeMove probeMove = new ProbeMove();
        final boolean debug;
        long lastEntryReceipt;
        final LockedReceipt lockedReceipt = new LockedReceipt();
        final RegistrationState registration = new RegistrationState();
        final Runnable timeout;
        GpuiGmsLocation gms;
        boolean gmsFallback;
        boolean restartAfterGms;
        String backend = "platform";
        Subscription(Activity activity, long id, int accuracy, double distance, boolean once, boolean background) {
            this.activity = new WeakReference<>(activity);
            this.application = activity.getApplication();
            this.manager = (LocationManager) activity.getSystemService(Context.LOCATION_SERVICE);
            this.id = id; this.accuracy = accuracy; this.distance = (float) distance; this.once = once;
            this.background = background;
            debug = (application.getApplicationInfo().flags & android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0;
            timeout = () -> { if (active) { emit("error|timeout|No fresh location before deadline"); finish("timeout"); } };
        }
        void emit(String event) { if (active) nativeEvent(id, event); }
        void finish(String reason) { emit("end|" + reason); stopOnMain(id); }
        void suspend() {
            suspended = true;
            restartAfterGms = false;
            if (gms != null) gms.stop();
            registration.removed();
            emit("paused|");
            if (manager != null) {
                try { manager.removeUpdates(this); }
                catch (SecurityException error) { emit("error|permission_denied|" + error.getMessage()); }
            }
        }
        void resume() {
            if (!active) return;
            if (background && !GpuiLocationService.isReady()) return;
            Context owner = background ? application : activity.get();
            if (owner == null) { finish("owner_destroyed"); return; }
            boolean fine = owner.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED;
            boolean coarse = owner.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED;
            emit("permission|" + (fine ? "precise" : coarse ? "approximate" : "denied"));
            if (!fine && !coarse) { suspend(); if (background) finish("permission_denied"); return; }
            if (gms != null) {
                if (gms.stopping()) { restartAfterGms = true; gms.stop(); }
                return;
            }
            if (GpuiGmsLocation.cleanupPending()) {
                suspended = true;
                emit("error|cleanup_pending|Previous GMS listener cleanup is pending; refresh after cleanup");
                return;
            }
            if (!gmsFallback) {
                try {
                    if (GpuiGmsLocation.available(application)) {
                        suspended = false;
                        backend = "gms_fused";
                        registeredAt = SystemClock.elapsedRealtime();
                        gms = new GpuiGmsLocation(application, new GpuiGmsLocation.Listener() {
                            @Override public void ready() {
                                if (!active || suspended) return;
                                registeredAt = SystemClock.elapsedRealtime();
                                android.util.Log.i("GpuiLocation", "listener_registered backend=gms_fused background=" + background);
                                emit("ready|" + (background ? "background" : "foreground") + "|gms_fused");
                            }
                            @Override public void location(Location location) { onLocationChanged(location); }
                            @Override public void closed(String reason) {
                                gms = null;
                                if (!active) return;
                                boolean restart = restartAfterGms;
                                restartAfterGms = false;
                                if (reason.equals("permission_denied")) {
                                    suspended = true;
                                    emit("error|permission_denied|GMS location permission denied");
                                    if (background) finish("permission_denied");
                                    return;
                                }
                                if (!reason.equals("stopped")) {
                                    gmsFallback = true;
                                    android.util.Log.i("GpuiLocation", "backend_fallback from=gms_fused reason=" + reason
                                        + " resolution=user_owned");
                                    restart = !suspended;
                                }
                                if (restart) resume();
                            }
                            @Override public void cleanupFailed() {
                                android.util.Log.e("GpuiLocation", "backend_cleanup_failed backend=gms_fused request=" + id);
                                emit("error|cleanup|GMS removal failed; refresh retries cleanup before restart");
                            }
                        });
                        gms.start(accuracy, distance, fine);
                        return;
                    }
                    gmsFallback = true;
                    android.util.Log.i("GpuiLocation", "backend_fallback from=gms_fused reason=services_unavailable");
                } catch (RuntimeException | LinkageError error) {
                    gmsFallback = true;
                    if (gms != null) { restartAfterGms = true; gms.stop(); return; }
                    android.util.Log.i("GpuiLocation", "backend_fallback from=gms_fused reason=client_unavailable");
                }
            }
            backend = "platform";
            if (manager == null) { suspend(); emit("error|unavailable|LocationManager unavailable"); return; }
            try {
                for (String provider : ProviderPolicy.candidates(android.os.Build.VERSION.SDK_INT >= 31, fine, accuracy >= 3)) {
                if (!manager.getAllProviders().contains(provider)) continue;
                if (!manager.isProviderEnabled(provider)) continue;
                if (background && registration.preserve(true, provider)) {
                    android.util.Log.i("GpuiLocation", "listener_preserved provider=" + provider + " background=true");
                    logPowerState("ready_preserved");
                    emit("ready|background|" + provider);
                    return;
                }
                suspend();
                try {
                    if (android.os.Build.VERSION.SDK_INT >= 31) {
                        android.location.LocationRequest request = new android.location.LocationRequest.Builder(1000)
                            .setQuality(accuracy >= 3 ? android.location.LocationRequest.QUALITY_HIGH_ACCURACY
                                : android.location.LocationRequest.QUALITY_BALANCED_POWER_ACCURACY)
                            .setMinUpdateIntervalMillis(1000).setMinUpdateDistanceMeters(distance).build();
                        manager.requestLocationUpdates(provider, request, application.getMainExecutor(), this);
                    } else manager.requestLocationUpdates(provider, 1000, distance, this, Looper.getMainLooper());
                } catch (IllegalArgumentException | IllegalStateException failure) {
                    manager.removeUpdates(this);
                    android.util.Log.i("GpuiLocation", "provider_request_failed provider=" + provider
                        + " category=" + failure.getClass().getSimpleName());
                    continue;
                }
                suspended = false;
                registeredAt = SystemClock.elapsedRealtime();
                registration.registered(provider);
                android.util.Log.i("GpuiLocation", "listener_registered provider=" + provider + " background=" + background);
                logPowerState("ready");
                emit("ready|" + (background ? "background" : "foreground") + "|platform_" + provider);
                return;
                }
                suspend(); emit("error|unavailable|No enabled provider accepted the location request; refresh after enabling");
            } catch (SecurityException error) {
                suspend();
                emit("permission|denied"); emit("error|permission_denied|" + error.getMessage());
                if (background) finish("permission_denied");
            } catch (IllegalArgumentException error) { suspend(); emit("error|native|" + error.getMessage()); }
        }
        void logPowerState(String phase) {
            // Diagnostic only. A failed status query must not alter listener ownership.
            try {
                android.os.PowerManager power = application.getSystemService(android.os.PowerManager.class);
                String mode = power == null || android.os.Build.VERSION.SDK_INT < 28
                    ? "unavailable" : Integer.toString(power.getLocationPowerSaveMode());
                android.util.Log.i("GpuiLocation", "power_state phase=" + phase
                    + " locationPowerSaveMode=" + mode
                    + " powerSaveEnabled=" + (power == null ? "unavailable" : Boolean.toString(power.isPowerSaveMode()))
                    + " gpsEnabled=" + (manager != null && manager.isProviderEnabled(LocationManager.GPS_PROVIDER))
                    + " networkEnabled=" + (manager != null && manager.isProviderEnabled(LocationManager.NETWORK_PROVIDER)));
            } catch (RuntimeException error) {
                android.util.Log.i("GpuiLocation", "power_state phase=" + phase + " unavailable=" + error.getClass().getSimpleName());
            }
        }
        @Override public void onLocationChanged(Location location) {
            if (debug) {
                long receipt = SystemClock.elapsedRealtimeNanos();
                String reason = callbackCounts.record(active, suspended, location.getElapsedRealtimeNanos(), requestedAt, lastFix,
                    Fix.valid(location.getLatitude(), location.getLongitude(), location.hasAccuracy(), location.getAccuracy(), location.getTime()));
                android.util.Log.i("GpuiLocation", "callback_entry request=" + id
                    + " receipt_elapsed_ms=" + receipt / 1000000
                    + " cadence_ms=" + (lastEntryReceipt == 0 ? -1 : (receipt - lastEntryReceipt) / 1000000)
                    + " fix_age_ms=" + (receipt - location.getElapsedRealtimeNanos()) / 1000000
                    + " result=" + reason + callbackCounts.summary());
                lastEntryReceipt = receipt;
            }
            if (!ProviderPolicy.deliver(active, suspended, location.getElapsedRealtimeNanos(), requestedAt, lastFix)) return;
            // Invalid fixes must not complete a one-shot or poison the cache stream.
            if (!Fix.valid(location.getLatitude(), location.getLongitude(), location.hasAccuracy(),
                location.getAccuracy(), location.getTime())) return;
            boolean resumed = HostState.resumed(activity.get());
            if (background && lockedReceipt.pending() && !resumed) {
                android.os.PowerManager power = application.getSystemService(android.os.PowerManager.class);
                android.app.KeyguardManager keyguard = application.getSystemService(android.app.KeyguardManager.class);
                if (power != null && keyguard != null) {
                    boolean interactive = power.isInteractive();
                    boolean locked = keyguard.isDeviceLocked();
                    if (lockedReceipt.record(resumed, interactive, locked)) {
                        android.util.Log.i("GpuiLocation", "background_fix epoch=" + lockedReceipt.epoch()
                            + " receipt_elapsed_ms=" + SystemClock.elapsedRealtime()
                            + " host_resumed=false interactive=false device_locked=true");
                    }
                }
            }
            if (!loggedFirstFix) {
                loggedFirstFix = true;
                android.util.Log.i("GpuiLocation", "first_valid_fix background=" + background);
            }
            long receipt = SystemClock.elapsedRealtime();
            android.util.Log.i("GpuiLocation", "live_fix provider=" + location.getProvider()
                + " backend=" + backend
                + " registration_elapsed_ms=" + (receipt - registeredAt)
                + " cadence_ms=" + (lastReceipt == 0 ? -1 : receipt - lastReceipt)
                + " age_ms=" + Math.max(0, (SystemClock.elapsedRealtimeNanos() - location.getElapsedRealtimeNanos()) / 1000000)
                + " accuracy_m=" + location.getAccuracy());
            lastReceipt = receipt;
            lastFix = location.getElapsedRealtimeNanos();
            if (debug) probeReceipt.record(location.getTime(), lastFix, SystemClock.elapsedRealtimeNanos(), resumed);
            emit("position|" + serialize(location));
            if (once) finish("completed");
        }
        @Override public void onProviderDisabled(String provider) {
            // resume reports disabled only when no permitted provider remains. Reporting
            // an error first would make Rust cancel a one-shot before fallback can deliver.
            if (active && !suspended) resume();
        }
        @Override public void onProviderEnabled(String provider) { if (!suspended) resume(); }
        @Override public void onStatusChanged(String provider, int status, Bundle extras) {}
        @Override public void onActivityResumed(Activity a) { if (activity.get() == a) setHostResumed(a, true); }
        @Override public void onActivityPaused(Activity a) { if (activity.get() == a) setHostResumed(a, false); }
        @Override public void onActivityDestroyed(Activity a) { if (!background && activity.get() == a) finish("owner_destroyed"); }
        @Override public void onActivityCreated(Activity a, Bundle state) {}
        @Override public void onActivityStarted(Activity a) {}
        @Override public void onActivityStopped(Activity a) {}
        @Override public void onActivitySaveInstanceState(Activity a, Bundle state) {}
    }
    private GpuiLocation() {}
}
