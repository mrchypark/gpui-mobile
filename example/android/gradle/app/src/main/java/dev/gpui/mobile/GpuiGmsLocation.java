package dev.gpui.mobile;

import android.content.Context;
import android.location.Location;
import android.os.Looper;
import android.os.Handler;
import com.google.android.gms.common.ConnectionResult;
import com.google.android.gms.common.GoogleApiAvailability;
import com.google.android.gms.common.api.ResolvableApiException;
import com.google.android.gms.common.api.ApiException;
import com.google.android.gms.location.*;
import java.util.ArrayList;
import java.util.Comparator;

/** One main-thread-owned GMS registration. Close completes before fallback/restart. */
final class GpuiGmsLocation {
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static final java.util.concurrent.Executor MAIN_EXECUTOR = command -> MAIN.post(command);
    // Logical Stop releases its FGS lease immediately; this separate owner retains
    // native cleanup until acknowledged and blocks replacement registrations.
    static final java.util.Map<GpuiGmsLocation, Integer> RETIRED = new java.util.IdentityHashMap<>();
    static void retire(GpuiGmsLocation attempt) {
        RETIRED.putIfAbsent(attempt, 0);
        attempt.stop();
    }
    static boolean cleanupPending() {
        for (GpuiGmsLocation attempt : new ArrayList<>(RETIRED.keySet())) attempt.stop();
        return !RETIRED.isEmpty();
    }
    private void removalFailed() {
        listener.cleanupFailed();
        Integer retries = RETIRED.get(this);
        if (retries != null && retries < 3) {
            RETIRED.put(this, retries + 1);
            MAIN.postDelayed(() -> { if (RETIRED.containsKey(this)) stop(); }, 1000);
        }
    }
    interface Listener {
        void ready();
        void location(Location location);
        void closed(String reason);
        void cleanupFailed();
    }
    static boolean available(Context context) {
        return GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(context) == ConnectionResult.SUCCESS;
    }
    static final class Ownership {
        boolean stopped, registering, registered, removing, closed;
        boolean accepts() { return !stopped && !closed; }
        boolean needsRemoval() { return stopped && (registering || registered) && !removing && !closed; }
    }
    private final FusedLocationProviderClient client;
    private final Context context;
    private final Listener listener;
    private final Ownership owner = new Ownership();
    private String reason = "stopped";
    private final LocationCallback callback = new LocationCallback() {
        @Override public void onLocationResult(LocationResult result) {
            if (!owner.accepts()) return;
            ArrayList<Location> ordered = new ArrayList<>(result.getLocations());
            ordered.sort(Comparator.comparingLong(Location::getElapsedRealtimeNanos));
            for (Location location : ordered) {
                if (!owner.accepts()) break;
                listener.location(location);
            }
        }
    };
    GpuiGmsLocation(Context context, Listener listener) {
        this.context = context;
        this.listener = listener;
        client = LocationServices.getFusedLocationProviderClient(context);
    }
    boolean stopping() { return owner.stopped; }
    boolean registered() { return owner.registered && !owner.registering && owner.accepts(); }
    void start(int accuracy, float distance, boolean fine) {
        int priority = !fine ? Priority.PRIORITY_BALANCED_POWER_ACCURACY
            : accuracy >= 3 ? Priority.PRIORITY_HIGH_ACCURACY
            : accuracy == 2 ? Priority.PRIORITY_BALANCED_POWER_ACCURACY : Priority.PRIORITY_LOW_POWER;
        LocationRequest request = new LocationRequest.Builder(priority, 1000)
            .setMinUpdateIntervalMillis(1000).setMinUpdateDistanceMeters(distance)
            .setMaxUpdateDelayMillis(0).setMaxUpdateAgeMillis(0)
            .setWaitForAccurateLocation(false).build();
        try {
            LocationServices.getSettingsClient(context).checkLocationSettings(
                new LocationSettingsRequest.Builder().addLocationRequest(request).build())
                .addOnCompleteListener(MAIN_EXECUTOR, task -> {
                    logSettings(task.isSuccessful(), task.getException(),
                        task.isSuccessful() ? task.getResult().getLocationSettingsStates() : null);
                    if (task.isSuccessful()) register(request);
                    else fail(task.getException() instanceof SecurityException
                        ? "permission_denied" : task.getException() instanceof ResolvableApiException
                        ? "settings_resolution_required" : "settings_unavailable");
                });
        } catch (SecurityException error) { fail("permission_denied"); }
        catch (RuntimeException error) { fail("settings_unavailable"); }
    }
    private void logSettings(boolean success, Exception error, LocationSettingsStates states) {
        // Diagnostics must neither resolve settings nor affect backend selection.
        try {
            if ((context.getApplicationInfo().flags & android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) == 0) return;
            android.util.Log.i("GpuiLocation", "gms_settings success=" + success
                + " status=" + (error instanceof ApiException ? Integer.toString(((ApiException) error).getStatusCode()) : success ? "0" : "unavailable")
                + " resolution_present=" + (error instanceof ResolvableApiException && ((ResolvableApiException) error).getResolution() != null)
                + settingsStates(states));
            LocationServices.getSettingsClient(context).isGoogleLocationAccuracyEnabled()
                .addOnCompleteListener(MAIN_EXECUTOR, task -> {
                    try {
                        android.util.Log.i("GpuiLocation", "gms_settings gla_enabled="
                            + (task.isSuccessful() ? String.valueOf(task.getResult()) : "unavailable"));
                    } catch (RuntimeException ignored) { }
                });
        } catch (RuntimeException ignored) { }
    }
    static String settingsStates(LocationSettingsStates s) {
        if (s == null) return " states=unavailable";
        return " gps_present=" + s.isGpsPresent() + " gps_usable=" + s.isGpsUsable()
            + " network_present=" + s.isNetworkLocationPresent() + " network_usable=" + s.isNetworkLocationUsable()
            + " ble_present=" + s.isBlePresent() + " ble_usable=" + s.isBleUsable()
            + " location_present=" + s.isLocationPresent() + " location_usable=" + s.isLocationUsable();
    }
    private void register(LocationRequest request) {
        if (!owner.accepts()) return;
        owner.registering = true;
        try {
            client.requestLocationUpdates(request, callback, Looper.getMainLooper())
                .addOnCompleteListener(MAIN_EXECUTOR, task -> {
                    owner.registering = false;
                    // Even failed registration is removed before allowing a second backend.
                    owner.registered = true;
                    if (owner.stopped) stop();
                    else if (!task.isSuccessful()) {
                        fail(task.getException() instanceof SecurityException ? "permission_denied" : "registration_failed");
                    } else listener.ready();
                });
        } catch (RuntimeException error) {
            owner.registering = false;
            owner.registered = true;
            if (owner.stopped) stop();
            else fail(error instanceof SecurityException ? "permission_denied" : "registration_failed");
        }
    }
    private void fail(String failure) {
        if (owner.closed || owner.stopped) return;
        reason = failure;
        stop();
    }
    void stop() {
        owner.stopped = true;
        if (owner.closed || owner.removing) return;
        if (!owner.registered && !owner.registering) { close(); return; }
        if (!owner.needsRemoval()) return;
        owner.removing = true;
        boolean registrationWasPending = owner.registering;
        try {
            client.removeLocationUpdates(callback).addOnCompleteListener(MAIN_EXECUTOR, task -> {
                owner.removing = false;
                if (task.isSuccessful()) {
                    owner.registered = registrationWasPending && !owner.registering;
                    // A pending request may register after this removal; its completion
                    // calls stop again. Until then, keep this attempt fenced and owned.
                    if (!owner.registering) {
                        if (owner.registered) stop();
                        else close();
                    }
                }
                else removalFailed();
            });
        } catch (RuntimeException error) { owner.removing = false; removalFailed(); }
    }
    private void close() {
        if (owner.closed) return;
        owner.closed = true;
        RETIRED.remove(this);
        listener.closed(reason);
    }
}
