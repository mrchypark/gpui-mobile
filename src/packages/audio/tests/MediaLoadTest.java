package dev.gpui.mobile;

/** Compile alongside production GpuiMediaLoad; requires no Android runtime. */
public final class MediaLoadTest {
    public static void main(String[] args) throws Exception {
        GpuiMediaLoad cancelled = new GpuiMediaLoad();
        RuntimeException disposal = new IllegalStateException("disposed");
        cancelled.complete(disposal);
        cancelled.complete(null); // A late prepared callback cannot erase cancellation.
        try {
            cancelled.await();
            throw new AssertionError("cancellation lost");
        } catch (RuntimeException actual) {
            if (actual != disposal) throw new AssertionError("wrong failure", actual);
        }

        GpuiMediaLoad next = new GpuiMediaLoad();
        java.util.concurrent.atomic.AtomicReference<Throwable> failure =
                new java.util.concurrent.atomic.AtomicReference<>();
        java.util.concurrent.CountDownLatch entered = new java.util.concurrent.CountDownLatch(1);
        Thread waiter = new Thread(() -> {
            entered.countDown();
            try { next.await(); } catch (Throwable error) { failure.set(error); }
        });
        waiter.start();
        entered.await();
        waiter.interrupt();
        waiter.join(2000);
        if (waiter.isAlive() || !(failure.get() instanceof IllegalStateException)) {
            throw new AssertionError("interrupted wait did not terminate", failure.get());
        }
        cancelled.complete(new IllegalStateException("stale callback"));
        next.complete(null);
        next.complete(disposal); // First completion also wins on success.
        next.await();
        System.out.println("MediaLoadTest PASS");
    }
}
