package dev.gpui.mobile;

import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import com.google.android.play.core.review.ReviewManager;
import com.google.android.play.core.review.ReviewManagerFactory;
import java.lang.ref.WeakReference;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

public final class GpuiInAppReview {
    private static final AtomicReference<Object> inFlight = new AtomicReference<>();

    /** Necessary Play Store prerequisite, not a promise of user/account eligibility or a dialog. */
    public static boolean isAvailable(Activity activity) {
        try {
            return activity.getPackageManager().getApplicationInfo("com.android.vending", 0).enabled;
        } catch (PackageManager.NameNotFoundException e) {
            return false;
        }
    }

    /** null on completed flow, otherwise an error; never opens the store listing. */
    public static String requestReview(Activity activity) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            return "request_review must run off the Android UI thread";
        }
        if (activity.isFinishing() || activity.isDestroyed()) return "Review Activity is not active";
        // Let Play determine eligibility; package visibility can hide an installed store.
        Object ticket = new Object();
        if (!inFlight.compareAndSet(null, ticket)) return "A review request is already in progress";
        CountDownLatch done = new CountDownLatch(1);
        AtomicBoolean cancelled = new AtomicBoolean();
        AtomicBoolean launched = new AtomicBoolean();
        AtomicReference<String> error = new AtomicReference<>();
        WeakReference<Activity> owner = new WeakReference<>(activity);
        ReviewManager manager;
        try {
            manager = ReviewManagerFactory.create(activity.getApplicationContext());
            new Handler(Looper.getMainLooper()).post(() -> {
                if (cancelled.get()) return;
                try {
                    manager.requestReviewFlow().addOnCompleteListener(request -> {
                        if (cancelled.get()) return;
                        if (!request.isSuccessful()) {
                            error.set("ReviewInfo request failed: " + request.getException());
                            done.countDown();
                            return;
                        }
                        Activity current = owner.get();
                        if (current == null || current.isFinishing() || current.isDestroyed()) {
                            error.set("Review Activity was destroyed");
                            done.countDown();
                            return;
                        }
                        try {
                            synchronized (cancelled) {
                                if (cancelled.get()) return;
                                launched.set(true);
                                manager.launchReviewFlow(current, request.getResult()).addOnCompleteListener(flow -> {
                                    if (!flow.isSuccessful()) error.set("Review flow failed: " + flow.getException());
                                    inFlight.compareAndSet(ticket, null);
                                    done.countDown();
                                });
                            }
                        } catch (RuntimeException failure) {
                            launched.set(false);
                            error.set("Review launch failed: " + failure);
                            done.countDown();
                        }
                    });
                } catch (RuntimeException failure) {
                    error.set("Review request failed: " + failure);
                    done.countDown();
                }
            });
            if (!done.await(2, TimeUnit.MINUTES)) {
                synchronized (cancelled) { cancelled.set(true); }
                return "Review flow timed out; dialog visibility is unknown";
            }
            return error.get();
        } catch (InterruptedException interrupted) {
            synchronized (cancelled) { cancelled.set(true); }
            Thread.currentThread().interrupt();
            return "Review request interrupted";
        } catch (RuntimeException failure) {
            synchronized (cancelled) { cancelled.set(true); }
            return "Review request failed: " + failure;
        } finally {
            // A timed-out, already-launched Play flow remains exclusive until its completion.
            if (!launched.get()) inFlight.compareAndSet(ticket, null);
        }
    }

    public static boolean openStoreListing(Activity activity, String appId) {
        try {
            Intent intent = new Intent(Intent.ACTION_VIEW,
                Uri.parse("market://details?id=" + Uri.encode(appId)));
            activity.startActivity(intent);
            return true;
        } catch (Exception e) {
            try {
                Intent intent = new Intent(Intent.ACTION_VIEW,
                    Uri.parse("https://play.google.com/store/apps/details?id=" + Uri.encode(appId)));
                activity.startActivity(intent);
                return true;
            } catch (Exception ex) {
                return false;
            }
        }
    }
    private GpuiInAppReview() {}
}
