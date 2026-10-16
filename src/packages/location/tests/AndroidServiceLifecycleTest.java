package dev.gpui.mobile;

/** Production service state, without starting Android services or native callbacks. */
public final class AndroidServiceLifecycleTest {
    public static void main(String[] args) {
        GpuiLocationService.Lifecycle state = new GpuiLocationService.Lifecycle();
        Object oldService = new Object(), newService = new Object();
        check(state.canAcquire() && !state.ready());
        state.started(oldService);
        check(state.ready());
        state.stopping(); // last lease released; framework destruction has not arrived
        check(!state.ready() && !state.canAcquire()); // reacquire must return retryable failure
        state.started(oldService); // a queued start command cannot resurrect a stopping owner
        check(!state.ready() && !state.canAcquire());
        check(state.destroyed(oldService));
        check(state.canAcquire() && !state.ready());
        state.started(newService); // deliberate retry after teardown
        check(state.ready());
        check(!state.destroyed(oldService)); // stale destroy cannot clear the replacement
        check(state.ready());
        state.stopping();
        check(state.destroyed(newService) && state.canAcquire() && !state.ready());
        System.out.println("PASS location service release/reacquire, queued-start, stale-destroy, retry");
    }
    private static void check(boolean condition) { if (!condition) throw new AssertionError(); }
}
