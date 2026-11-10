package dev.gpui.mobile.example;

import android.app.Activity;
import android.app.Application;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Bundle;
import android.os.SystemClock;
import android.view.View;

import java.lang.reflect.Method;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/** Exercises the native Activity lifecycle without replacing the app's stored data. */
public final class ActivityLifecycleTest extends Instrumentation {
    private static final String ACTIVITY = "dev.gpui.mobile.GpuiActivity";
    private static final long TIMEOUT_MS = 30000;
    private String scenario;

    @Override
    public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        scenario = arguments.getString("scenario", "recreate");
        start();
    }

    @Override
    public void onStart() {
        Bundle result = new Bundle();
        try {
            Intent launch = new Intent(Intent.ACTION_MAIN)
                    .setClassName(getTargetContext().getPackageName(), ACTIVITY)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            Activity activity = startActivitySync(launch);
            awaitReady(activity);
            if ("recreate".equals(scenario) || "recreate_stress".equals(scenario)) {
                recreate(activity, "recreate_stress".equals(scenario));
            } else if ("background".equals(scenario)) {
                backgroundResume(activity, launch);
            } else if ("insets_surface".equals(scenario)) {
                insetsSurface(activity);
            } else {
                throw new IllegalArgumentException("Unknown scenario: " + scenario);
            }
            result.putString("stream", "PASS: " + scenario + "\n");
            finish(Activity.RESULT_OK, result);
        } catch (Throwable failure) {
            result.putString("stream", "FAIL: " + android.util.Log.getStackTraceString(failure));
            finish(Activity.RESULT_CANCELED, result);
        }
    }

    private void insetsSurface(Activity activity) throws Exception {
        Class<?> observer = Class.forName("dev.gpui.mobile.GpuiInsets");
        Method find = observer.getDeclaredMethod("findSurface", View.class);
        find.setAccessible(true);
        AtomicReference<View> found = new AtomicReference<>();
        AtomicReference<Throwable> failure = new AtomicReference<>();
        runOnMainSync(() -> {
            try {
                found.set((View) find.invoke(null, activity.getWindow().getDecorView()));
            } catch (Throwable error) {
                failure.set(error);
            }
        });
        if (failure.get() != null) throw new AssertionError(failure.get());
        if (found.get() != activity.getWindow().getDecorView()) {
            throw new AssertionError("NativeActivity insets must use the rendering window");
        }
    }

    private void recreate(Activity activity, boolean stress) throws Exception {
        int pid = android.os.Process.myPid();
        // Each new Activity must have a usable native window in the same process.
        for (int generation = 1; generation <= 3; generation++) {
            Activity previous = activity;
            AtomicReference<Activity> next = new AtomicReference<>();
            CountDownLatch created = new CountDownLatch(1);
            Application application = previous.getApplication();
            Application.ActivityLifecycleCallbacks callbacks = new Application.ActivityLifecycleCallbacks() {
                @Override public void onActivityCreated(Activity candidate, Bundle state) {
                    if (candidate != previous && candidate.getClass().getName().equals(ACTIVITY)) {
                        next.set(candidate);
                        created.countDown();
                    }
                }
                @Override public void onActivityStarted(Activity a) {}
                @Override public void onActivityResumed(Activity a) {}
                @Override public void onActivityPaused(Activity a) {}
                @Override public void onActivityStopped(Activity a) {}
                @Override public void onActivitySaveInstanceState(Activity a, Bundle out) {}
                @Override public void onActivityDestroyed(Activity a) {}
            };
            try {
                application.registerActivityLifecycleCallbacks(callbacks);
                runOnMainSync(previous::recreate);
                if (!created.await(TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
                    throw new AssertionError("Activity was not recreated");
                }
                activity = next.get();
                if (activity == null || activity == previous) {
                    throw new AssertionError("Activity was not recreated");
                }
                awaitReady(activity);
                if (android.os.Process.myPid() != pid) {
                    throw new AssertionError("Process restarted during recreation");
                }
            } finally {
                application.unregisterActivityLifecycleCallbacks(callbacks);
            }
        }
        if (stress) {
            SystemClock.sleep(5000);
            awaitReady(activity);
        }
        Activity last = activity;
        runOnMainSync(last::finish);
    }

    private void backgroundResume(Activity activity, Intent launch) throws Exception {
        Intent home = new Intent(Intent.ACTION_MAIN)
                .addCategory(Intent.CATEGORY_HOME)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        getTargetContext().startActivity(home);
        long deadline = SystemClock.uptimeMillis() + TIMEOUT_MS;
        boolean[] focused = new boolean[1];
        do {
            runOnMainSync(() -> focused[0] = activity.hasWindowFocus());
            if (!focused[0]) {
                break;
            }
            SystemClock.sleep(50);
        } while (SystemClock.uptimeMillis() < deadline);
        if (focused[0]) {
            throw new AssertionError("Activity did not enter background");
        }
        SystemClock.sleep(1500);
        getTargetContext().startActivity(launch);
        awaitReady(activity);
        runOnMainSync(activity::finish);
    }

    private void awaitReady(Activity activity) throws Exception {
        Method ready = activity.getClass().getDeclaredMethod("isNativeReady");
        ready.setAccessible(true);
        Method nativeFocus = activity.getClass().getDeclaredMethod("nativeHasWindowFocus");
        nativeFocus.setAccessible(true);
        long deadline = SystemClock.uptimeMillis() + TIMEOUT_MS;
        boolean[] usable = new boolean[1];
        do {
            runOnMainSync(() -> {
                try {
                    usable[0] = !activity.isDestroyed() && activity.hasWindowFocus()
                            && (Boolean) ready.invoke(activity) && (Boolean) nativeFocus.invoke(null);
                } catch (ReflectiveOperationException error) {
                    throw new AssertionError(error);
                }
            });
            if (usable[0]) {
                // Give the native loop time to surface a crash after initialization.
                SystemClock.sleep(1000);
                return;
            }
            SystemClock.sleep(50);
        } while (SystemClock.uptimeMillis() < deadline);
        throw new AssertionError("Native window did not become ready");
    }
}
