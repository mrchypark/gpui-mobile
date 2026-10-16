package dev.gpui.mobile;

import android.app.Activity;
import android.content.Intent;
import android.content.Context;
import android.os.Bundle;
import android.os.Looper;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicReference;

/** One native permission/settings request owns its result through completion. */
public class GpuiPermissionActivity extends Activity {
    private static final int CODE = 9002;
    private static final class Request {
        final String permission;
        final Intent settings;
        final CountDownLatch done = new CountDownLatch(1);
        int result = GpuiPermissions.DENIED;
        RuntimeException error;
        Request(String permission, Intent settings) { this.permission = permission; this.settings = settings; }
    }
    private static final AtomicReference<Request> ACTIVE = new AtomicReference<>();
    private Request pending;

    static int request(Activity activity, String permission, Intent settings) {
        if (Looper.myLooper() == Looper.getMainLooper()) throw new IllegalStateException("Request permissions off the Android UI thread");
        Request request = new Request(permission, settings);
        if (!ACTIVE.compareAndSet(null, request)) throw new IllegalStateException("Another permission request is active");
        try {
            Intent proxy = new Intent(activity, GpuiPermissionActivity.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            activity.startActivity(proxy);
        } catch (RuntimeException error) { ACTIVE.compareAndSet(request, null); throw error; }
        boolean interrupted = false;
        for (;;) {
            try { request.done.await(); break; }
            catch (InterruptedException error) { interrupted = true; }
        }
        if (interrupted) {
            Thread.currentThread().interrupt();
            throw new IllegalStateException("Permission wait interrupted; native request has finished");
        }
        if (request.error != null) throw request.error;
        return request.result;
    }

    @Override protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        pending = ACTIVE.get();
        if (pending == null) { finish(); return; }
        if (saved != null) return;
        try {
            if (pending.settings != null) startActivityForResult(pending.settings, CODE);
            else requestPermissions(GpuiPermissions.runtimePermissions(pending.permission), CODE);
        } catch (RuntimeException error) { complete(error); }
    }

    @Override protected void onActivityResult(int code, int result, Intent data) {
        super.onActivityResult(code, result, data);
        if (code == CODE) complete(null); // Query real setting after the user returns.
    }

    @Override public void onRequestPermissionsResult(int code, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(code, permissions, results);
        if (code != CODE) return;
        if (results.length == 0) { completeCancelled(); return; }
        if (results.length != permissions.length) { complete(new IllegalStateException("Incomplete permission callback")); return; }
        android.content.SharedPreferences.Editor history = getSharedPreferences(GpuiPermissions.HISTORY, Context.MODE_PRIVATE).edit();
        for (String permission : permissions) history.putBoolean(permission, true);
        history.apply();
        complete(null);
    }

    private void completeCancelled() {
        if (pending != null && pending.done.getCount() != 0) {
            pending.result = GpuiPermissions.DENIED;
            ACTIVE.compareAndSet(pending, null);
            pending.done.countDown();
        }
        finish();
    }

    private void complete(RuntimeException error) {
        if (pending != null && pending.done.getCount() != 0) {
            try { if (error == null) pending.result = GpuiPermissions.checkPermission(this, pending.permission); }
            catch (RuntimeException failure) { error = failure; }
            pending.error = error;
            ACTIVE.compareAndSet(pending, null);
            pending.done.countDown();
        }
        finish();
    }

    @Override public void onBackPressed() { completeCancelled(); }
    @Override protected void onDestroy() {
        if (isFinishing() && pending != null && pending.done.getCount() != 0) {
            complete(new IllegalStateException("Permission activity closed before completion"));
        }
        super.onDestroy();
    }
}
