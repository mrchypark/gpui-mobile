package dev.gpui.mobile;

/** One native prepare attempt. Late callbacks cannot complete a later attempt. */
final class GpuiMediaLoad {
    private final java.util.concurrent.CountDownLatch done = new java.util.concurrent.CountDownLatch(1);
    private final java.util.concurrent.atomic.AtomicBoolean completed = new java.util.concurrent.atomic.AtomicBoolean();
    private volatile RuntimeException error;
    void complete(RuntimeException error) {
        if (completed.compareAndSet(false, true)) { this.error = error; done.countDown(); }
    }
    void await() {
        try {
            if (!done.await(30, java.util.concurrent.TimeUnit.SECONDS)) {
                throw new IllegalStateException("Media preparation timed out");
            }
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            throw new IllegalStateException("Media preparation interrupted", error);
        }
        if (error != null) throw error;
    }
}
