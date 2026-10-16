package dev.gpui.mobile;

import android.app.Activity;
import android.os.Handler;
import android.os.Looper;
import java.lang.reflect.Method;
import java.lang.reflect.InvocationTargetException;
import java.util.concurrent.Callable;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/** Run on a dedicated worker against real Android. Intentionally holds main
 * for the bounded timeout to exercise the lifecycle wait cycle; never run in production.
 */
public final class GpuiMediaSessionQueueNativeTest {
    private static void await(CountDownLatch latch) {
        try {
            if (!latch.await(8, TimeUnit.SECONDS)) throw new AssertionError("fixture timeout");
        } catch (InterruptedException error) { throw new AssertionError(error); }
    }
    public static void run(Activity activity) throws Exception {
        if (Looper.myLooper() != null) throw new AssertionError("Requires Looper-less worker");
        Handler main = new Handler(Looper.getMainLooper());
        GpuiMediaSession.release();
        CountDownLatch entered = new CountDownLatch(1);
        CountDownLatch unblock = new CountDownLatch(1);
        CountDownLatch drained = new CountDownLatch(1);
        main.post(() -> { entered.countDown(); await(unblock); });
        await(entered);
        try {
            long start = System.nanoTime();
            try {
                GpuiMediaSession.init(activity);
                throw new AssertionError("blocked queue reported success");
            } catch (IllegalStateException expected) { }
            if (System.nanoTime() - start > TimeUnit.SECONDS.toNanos(4))
                throw new AssertionError("unbounded main wait");
        } finally {
            unblock.countDown();
            main.post(drained::countDown);
            await(drained);
        }
        if (GpuiMediaSession.getSessionToken() != null) throw new AssertionError("late queued init orphan");

        // Exercise abandonment AFTER native session creation, before completion.
        Method onMain = GpuiMediaSession.class.getDeclaredMethod("onMain", Callable.class);
        onMain.setAccessible(true);
        CountDownLatch finish = new CountDownLatch(1);
        CountDownLatch cleaned = new CountDownLatch(1);
        try {
            onMain.invoke(null, (Callable<Void>) () -> {
                GpuiMediaSession.init(activity);
                await(finish);
                return null;
            });
            throw new AssertionError("running timeout reported success");
        } catch (InvocationTargetException expected) {
            if (!(expected.getCause() instanceof IllegalStateException)) throw expected;
        } finally {
            finish.countDown();
            main.post(cleaned::countDown);
            await(cleaned);
        }
        if (GpuiMediaSession.getSessionToken() != null) throw new AssertionError("running init orphan");
        // Teardown release executes inline on main, without waiting for worker.
        CountDownLatch released = new CountDownLatch(1);
        main.post(() -> { try { GpuiMediaSession.release(); } finally { released.countDown(); } });
        await(released);
        GpuiMediaSession.init(activity);
        GpuiMediaSession.release();
    }
}
