package dev.gpui.mobile;

import android.app.Activity;
import android.os.Looper;
import android.view.View;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicInteger;

/** Native assertion fixture: invoke run(activity) on a worker with a running UI loop. */
public final class NativeControlsTest {
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
    private static void success(String error) { check(error == null, String.valueOf(error)); }
    private static String encode(String text) {
        return text.replace("%", "%25").replace("|", "%7C").replace("=", "%3D");
    }
    public static void run(Activity activity) throws Exception {
        check(Looper.myLooper() != Looper.getMainLooper(), "Run fixture on a worker");
        dispatchStates();
        lifecycleQueueCancellation(activity);
        long first = Long.MIN_VALUE + 101, second = Long.MIN_VALUE + 102;
        String params = "html=" + encode("<html><body>pipes | percent % equals =</body></html>");
        try {
            success(GpuiPlatformView.createViewChecked(activity, "webview", first, 0, 0, 100, 100, params));
            success(GpuiPlatformView.createViewChecked(activity, "webview", second, 100, 0, 100, 100, params));
            success(GpuiPlatformView.command(first, "webview.stop", ""));
            success(GpuiPlatformView.command(second, "webview.stop", ""));
            GpuiPlatformView.setVisible(first, false);
            GpuiPlatformView.pauseAll();
            GpuiPlatformView.setVisible(second, true);
            onUi(activity, () -> check(GpuiPlatformView.getContainer(second).getVisibility() != View.VISIBLE,
                "Visibility changes while paused must stay suppressed"));
            GpuiPlatformView.resumeAll();
            onUi(activity, () -> {
                check(GpuiPlatformView.getContainer(first).getVisibility() == View.GONE, "Hidden view resurfaced");
                check(GpuiPlatformView.getContainer(second).getVisibility() == View.VISIBLE, "Visible view not restored");
            });
            success(GpuiPlatformView.command(first, "webview.evaluate", encode("window.instanceTag = 'first'")));
            success(GpuiPlatformView.command(second, "webview.evaluate", encode("window.instanceTag = 'second'")));
            success(GpuiPlatformView.command(first, "webview.evaluate", encode("if(window.instanceTag !== 'first') throw Error('cross-instance control')")));
            success(GpuiPlatformView.command(second, "webview.evaluate", encode("if(window.instanceTag !== 'second') throw Error('cross-instance control')")));
            check(GpuiPlatformView.command(first, "webview.evaluate", encode("throw Error('expected failure')")) != null,
                "JavaScript exceptions must reach Rust as errors");
            check(GpuiPlatformView.command(first, "map.clear", "") != null, "Reject wrong native type");
            GpuiPlatformView.disposeView(first);
            check(GpuiPlatformView.command(first, "webview.reload", "") != null, "Reject disposed instance");
            success(GpuiPlatformView.command(second, "webview.evaluate", encode("if(window.instanceTag !== 'second') throw Error('disposed wrong view')")));
        } finally {
            GpuiPlatformView.disposeView(first);
            GpuiPlatformView.disposeView(second);
            onUi(activity, () -> {
                check(GpuiPlatformView.getView(first) == null, "First view cleanup incomplete");
                check(GpuiPlatformView.getView(second) == null, "Second view cleanup incomplete");
            });
        }
    }

    private static void onUi(Activity activity, Runnable action) throws Exception {
        java.util.concurrent.CompletableFuture<Void> completion = new java.util.concurrent.CompletableFuture<>();
        activity.runOnUiThread(() -> {
            try { action.run(); completion.complete(null); }
            catch (Throwable error) { completion.completeExceptionally(error); }
        });
        completion.get(3, TimeUnit.SECONDS);
    }

    // Model the NativeActivity cycle: UI waits for native acknowledgement while native creates a view.
    private static void lifecycleQueueCancellation(Activity activity) throws Exception {
        CountDownLatch uiWaiting = new CountDownLatch(1), nativeAcknowledged = new CountDownLatch(1);
        long id = Long.MIN_VALUE + 103;
        activity.runOnUiThread(() -> {
            uiWaiting.countDown();
            try { nativeAcknowledged.await(3, TimeUnit.SECONDS); }
            catch (InterruptedException e) { Thread.currentThread().interrupt(); }
        });
        try {
            check(uiWaiting.await(3, TimeUnit.SECONDS), "UI did not enter lifecycle wait");
            String error = GpuiPlatformView.createViewChecked(activity, "container", id, 0, 0, 1, 1, "");
            check(error != null && error.contains("before execution"), "Blocked UI creation must cancel");
        } finally { nativeAcknowledged.countDown(); }
        try {
            onUi(activity, () -> check(GpuiPlatformView.getView(id) == null, "Cancelled creation ran late"));
        } finally { GpuiPlatformView.disposeView(id); }
    }

    private static void dispatchStates() throws Exception {
        AtomicInteger effects = new AtomicInteger(), cleanup = new AtomicInteger();
        GpuiPlatformView.MainCall<Integer> queued = new GpuiPlatformView.MainCall<>(effects::incrementAndGet,
            cleanup::incrementAndGet);
        try { queued.await(1); throw new AssertionError("Expected queued timeout"); }
        catch (TimeoutException expected) {}
        queued.run();
        check(effects.get() == 0 && cleanup.get() == 0, "Cancelled task produced effects");

        CountDownLatch started = new CountDownLatch(1), release = new CountDownLatch(1);
        GpuiPlatformView.MainCall<Integer> running = new GpuiPlatformView.MainCall<>(() -> {
            started.countDown();
            if (!release.await(3, TimeUnit.SECONDS)) throw new AssertionError("Test release missing");
            return effects.incrementAndGet();
        }, cleanup::incrementAndGet);
        Thread worker = new Thread(running, "platform-view-late-completion-test");
        worker.start();
        try {
            check(started.await(3, TimeUnit.SECONDS), "Task did not start");
            try { running.await(1); throw new AssertionError("Expected running timeout"); }
            catch (TimeoutException expected) {}
        } finally { release.countDown(); worker.join(3000); }
        check(!worker.isAlive(), "Test worker did not stop");
        check(effects.get() == 1 && cleanup.get() == 1, "Late work must retire its effects exactly once");
        running.run();
        check(effects.get() == 1 && cleanup.get() == 1, "Late task reran");
    }
}
