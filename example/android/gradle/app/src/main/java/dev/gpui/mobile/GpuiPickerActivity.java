package dev.gpui.mobile;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.net.Uri;
import android.os.Bundle;
import android.os.Looper;
import android.provider.MediaStore;
import java.io.File;

import java.util.ArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Transparent helper Activity that handles startActivityForResult calls.
 *
 * <p>NativeActivity cannot easily receive activity results, so this lightweight
 * transparent Activity is used as a proxy. It launches the requested intent,
 * captures the result, stores it in a static field, and finishes itself.</p>
 *
 * <p>Handles process death: when Android kills the process while the system
 * picker is in the foreground, this Activity is recreated from savedInstanceState.
 * The picker result is saved to SharedPreferences so it can be retrieved after
 * the app fully restarts.</p>
 *
 * <p>Called from Rust via JNI through GpuiFilePicker / GpuiImagePicker.</p>
 */
public class GpuiPickerActivity extends Activity {

    private static final int REQUEST_CODE = 9001;
    private static final String KEY_WAITING = "gpui_waiting_for_result";
    static final String PREFS_NAME = "gpui_picker_prefs";
    static final String PREF_PENDING_RESULT = "pending_result";
    static final String PREF_HAS_PENDING = "has_pending_result";

    private static final class Request {
        final String token = java.util.UUID.randomUUID().toString();
        final Intent intent;
        final boolean importFiles;
        final File captureFile;
        final CountDownLatch latch = new CountDownLatch(1);
        ArrayList<String> result;
        RuntimeException error;
        volatile boolean completed;
        volatile boolean importing;
        volatile GpuiPickerActivity owner;
        Request(Intent intent, boolean importFiles, File captureFile) {
            this.intent = intent; this.importFiles = importFiles; this.captureFile = captureFile;
        }
    }
    private static final AtomicReference<Request> ACTIVE = new AtomicReference<>();
    private Request request;

    static ArrayList<String> launch(Activity activity, Intent intent) {
        return launch(activity, intent, false, null);
    }

    static ArrayList<String> launch(Activity activity, Intent intent, boolean importFiles, File captureFile) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            throw new IllegalStateException("Blocking picker calls must run off the Android UI thread");
        }
        Request pending = new Request(intent, importFiles, captureFile);
        if (!ACTIVE.compareAndSet(null, pending)) {
            throw new IllegalStateException("Another picker request is active");
        }
        try {
            Intent proxy = new Intent(activity, GpuiPickerActivity.class);
            proxy.putExtra("gpui_request_token", pending.token);
            proxy.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            activity.startActivity(proxy);
        } catch (RuntimeException error) {
            ACTIVE.compareAndSet(pending, null);
            throw error;
        }
        boolean interrupted = false;
        for (;;) {
            try { pending.latch.await(); break; }
            catch (InterruptedException error) { interrupted = true; }
        }
        if (interrupted) {
            Thread.currentThread().interrupt();
            if (pending.importFiles && pending.result != null) {
                for (String path : pending.result) GpuiPickerFiles.discard(new File(path));
            }
            throw new IllegalStateException("Picker wait interrupted; native request has finished");
        }
        if (pending.error != null) throw pending.error;
        return pending.result;
    }

    /** Whether we are waiting for an onActivityResult callback. */
    private boolean mWaitingForResult = false;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        request = ACTIVE.get();
        if (request != null && !request.token.equals(getIntent().getStringExtra("gpui_request_token"))) request = null;
        if (request != null) {
            request.owner = this;
            if (request.importing) return;
        }

        if (savedInstanceState != null && savedInstanceState.getBoolean(KEY_WAITING, false)) {
            // Process was killed while the system picker was showing.
            // The system will recreate us and deliver the picker result via
            // onActivityResult. Just wait for it — don't try to launch again.
            android.util.Log.i("GpuiPicker", "Recreated after process death, waiting for result");
            mWaitingForResult = true;
            return;
        }

        if (request != null) {
            try {
                mWaitingForResult = true;
                startActivityForResult(request.intent, REQUEST_CODE);
            } catch (Exception e) {
                android.util.Log.e("GpuiPicker", "Failed to start picker intent", e);
                mWaitingForResult = false;
                deliverError(new IllegalStateException("Failed to start native picker", e));
                finish();
            }
        } else {
            deliverError(new IllegalStateException("Missing picker request"));
            finish();
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        super.onSaveInstanceState(outState);
        outState.putBoolean(KEY_WAITING, mWaitingForResult);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQUEST_CODE) return;
        if (request != null && (request.completed || request.importing)) return;
        try { handleResult(resultCode, data); }
        catch (RuntimeException error) { deliverError(error); finish(); }
    }

    private void handleResult(int resultCode, Intent data) {
        mWaitingForResult = false;

        ArrayList<String> uris = null;
        if (resultCode == RESULT_OK && data != null) {
            uris = extractUris(data);
        }
        if (resultCode == RESULT_OK && request != null && request.captureFile != null) {
            Uri output = request.intent.getParcelableExtra(MediaStore.EXTRA_OUTPUT);
            if (output == null) throw new IllegalStateException("Camera output URI missing");
            uris = new ArrayList<>();
            uris.add(output.toString());
        }

        if (resultCode == RESULT_OK && (uris == null || uris.isEmpty())) {
            deliverError(new IllegalStateException("Picker completed without a readable URI"));
            finish();
            return;
        }

        if (uris != null && request != null && request.importFiles) {
            final ArrayList<String> selected = uris;
            final Request importing = request;
            importing.importing = true;
            new Thread(() -> {
                ArrayList<String> imported = new ArrayList<>();
                try {
                    for (String uri : selected) {
                        imported.add(GpuiPickerFiles.importUri(this, Uri.parse(uri)).getAbsolutePath());
                    }
                    runOnUiThread(() -> { complete(importing, imported, null); finishOwner(importing); });
                } catch (RuntimeException error) {
                    for (String path : imported) {
                        try { GpuiPickerFiles.discard(new File(path)); }
                        catch (RuntimeException cleanup) { error.addSuppressed(cleanup); }
                    }
                    runOnUiThread(() -> { complete(importing, null, error); finishOwner(importing); });
                }
            }, "gpui-picker-import").start();
        } else {
            if (uris != null && data != null) {
                int flags = data.getFlags() & (Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
                if ((data.getFlags() & Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION) == 0) {
                    deliverError(new IllegalStateException("Provider did not grant durable document access"));
                    finish();
                    return;
                }
                try {
                    for (String value : uris) getContentResolver().takePersistableUriPermission(Uri.parse(value), flags);
                } catch (RuntimeException error) { deliverError(error); finish(); return; }
            }
            deliverResult(uris);
            finish();
        }
    }

    @Override
    public void onBackPressed() {
        if (request != null && request.importing) return;
        mWaitingForResult = false;
        deliverResult(null);
        super.onBackPressed();
    }

    @Override protected void onDestroy() {
        if (!isChangingConfigurations() && request != null && request.owner == this) {
            complete(request, null, new IllegalStateException("Picker activity closed before completion"));
        }
        super.onDestroy();
    }

    private static void finishOwner(Request pending) {
        GpuiPickerActivity owner = pending.owner;
        if (owner != null && !owner.isDestroyed()) owner.finish();
    }

    private static void discardImports(ArrayList<String> paths, RuntimeException error) {
        if (paths == null) return;
        for (String path : paths) {
            try { GpuiPickerFiles.discard(new File(path)); }
            catch (RuntimeException cleanup) {
                if (error != null) error.addSuppressed(cleanup);
                else android.util.Log.e("GpuiPicker", "Cannot reclaim late import", cleanup);
            }
        }
    }

    // All terminal paths, including a late importer, converge on this request-bound gate.
    private static void complete(Request pending, ArrayList<String> paths, RuntimeException error) {
        synchronized (pending) {
            if (pending.completed) {
                if (pending.importFiles) discardImports(paths, error);
                return;
            }
            if (pending.captureFile != null && pending.captureFile.exists() && !pending.captureFile.delete()) {
                RuntimeException cleanup = new IllegalStateException("Cannot remove temporary camera output");
                if (error == null) error = cleanup; else error.addSuppressed(cleanup);
            }
            if (error != null && pending.importFiles) discardImports(paths, error);
            pending.result = error == null ? paths : null;
            pending.error = error;
            pending.completed = true;
            ACTIVE.compareAndSet(pending, null);
            pending.latch.countDown();
        }
    }

    /**
     * Deliver the result to the waiting Rust thread, or save to SharedPreferences
     * if the process was recreated without an in-memory request.
     */
    private void deliverResult(ArrayList<String> uris) {
        if (request != null) {
            complete(request, uris, null);
        } else {
            // Process death recovery: save to SharedPreferences for later retrieval.
            android.util.Log.i("GpuiPicker", "No latch (process death recovery), saving to prefs");
            savePendingResult(uris);
        }
    }

    private void deliverError(RuntimeException error) {
        if (request != null) {
            complete(request, null, error);
        }
    }

    /**
     * Extract URIs from the picker result intent.
     */
    private static ArrayList<String> extractUris(Intent data) {
        ArrayList<String> uris = new ArrayList<>();
        if (data.getClipData() != null) {
            int count = data.getClipData().getItemCount();
            for (int i = 0; i < count; i++) {
                Uri uri = data.getClipData().getItemAt(i).getUri();
                if (uri != null) {
                    uris.add(uri.toString());
                }
            }
        } else if (data.getData() != null) {
            uris.add(data.getData().toString());
        }
        return uris;
    }

    /**
     * Save picker result to SharedPreferences so it survives process death.
     */
    private void savePendingResult(ArrayList<String> uris) {
        SharedPreferences prefs = getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE);
        SharedPreferences.Editor editor = prefs.edit();
        if (uris != null && !uris.isEmpty()) {
            // Join with \n separator — URIs don't contain newlines.
            StringBuilder sb = new StringBuilder();
            for (int i = 0; i < uris.size(); i++) {
                if (i > 0) sb.append('\n');
                sb.append(uris.get(i));
            }
            editor.putBoolean(PREF_HAS_PENDING, true);
            editor.putString(PREF_PENDING_RESULT, sb.toString());
        } else {
            editor.putBoolean(PREF_HAS_PENDING, false);
            editor.remove(PREF_PENDING_RESULT);
        }
        editor.apply();
    }

    // ── Static helpers for Rust JNI access ──────────────────────────────

    /**
     * Check if there is a pending picker result from a previous process death.
     * Called from Rust via JNI.
     *
     * @param activity The current Activity context.
     * @return Array of URI strings, or null if no pending result.
     */
    public static String[] getPendingResult(Activity activity) {
        SharedPreferences prefs = activity.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE);
        if (!prefs.getBoolean(PREF_HAS_PENDING, false)) {
            return null;
        }
        String result = prefs.getString(PREF_PENDING_RESULT, null);
        // Clear the pending result.
        prefs.edit()
            .putBoolean(PREF_HAS_PENDING, false)
            .remove(PREF_PENDING_RESULT)
            .apply();

        if (result == null || result.isEmpty()) {
            return null;
        }
        return result.split("\n");
    }

    /**
     * Clear any pending picker result.
     * Called from Rust via JNI.
     */
    public static void clearPendingResult(Activity activity) {
        activity.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            .edit()
            .putBoolean(PREF_HAS_PENDING, false)
            .remove(PREF_PENDING_RESULT)
            .apply();
    }
}
