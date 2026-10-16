package dev.gpui.mobile;

/** Executes the production fix filter without initializing Android's Handler/native bridge. */
public final class AndroidFixTest {
    public static void main(String[] args) {
        for (boolean gms : new boolean[]{false, true}) {
            int backend = GpuiLocation.ProbeBackend.flags(true, false, gms, !gms);
            check(backend == (gms ? 34 : 130));
            check(GpuiLocation.ProbeBackend.phase(backend | 1 | 4, true));
            check(GpuiLocation.ProbeBackend.phase(backend | 1 | 8, false));
            check(!GpuiLocation.ProbeBackend.phase(backend | 1 | 8 | 4, false));
            check(!GpuiLocation.ProbeBackend.phase(backend | 1, false));
            check(!GpuiLocation.ProbeBackend.phase(backend | 4, true));
            check(GpuiLocation.ProbeBackend.flags(false, false, gms, !gms) == 0);
            check(GpuiLocation.ProbeBackend.flags(true, true, gms, !gms) == 0);
        }
        check(GpuiLocation.ProbeBackend.flags(true, false, false, false) == 0);
        check(GpuiLocation.ProbeBackend.flags(true, false, true, true) == 0);
        check(!GpuiLocation.ProbeBackend.phase(1 | 2 | 4 | 32 | 128, true));
        check(!GpuiLocation.ProbeBackend.phase(-1, true));
        GpuiLocation.ProbeMove move = new GpuiLocation.ProbeMove();
        int[] moves = {0};
        java.util.function.BooleanSupplier action = () -> { moves[0]++; return true; };
        check(!move.move(false, true, true, action)); // no active debug route
        check(!move.move(true, false, true, action)); // stale/unowned token
        check(!move.move(true, true, false, action)); // not actually resumed
        check(moves[0] == 0);
        check(move.move(true, true, true, action));
        check(!move.move(true, true, true, action) && moves[0] == 1);
        GpuiLocation.ProbeMove failedMove = new GpuiLocation.ProbeMove();
        check(!failedMove.move(true, true, true, () -> false));
        check(failedMove.attempted); // report failure; do not loop lifecycle actions
        GpuiLocation.ProbeReceipt phase = new GpuiLocation.ProbeReceipt();
        phase.record(1000, 90, 95, true);
        check(!phase.matches(1000, 100, true)); // queued pre-resume sample consumed later
        check(!phase.matches(1000, 100, false)); // queued foreground sample after pause
        phase.record(1001, 99, 110, false);
        check(!phase.matches(1001, 100, false)); // native receipt new, fix predates phase
        phase.record(1002, 111, 112, false);
        check(phase.matches(1002, 100, false));
        check(!phase.matches(1001, 100, false)); // old Rust snapshot, newer native sample
        check(!phase.matches(1002, 100, true));
        phase.record(1002, 113, 114, false);
        check(!phase.matches(1002, 100, false)); // same wall timestamp is ambiguous
        phase.record(999, 115, 116, false);
        check(!phase.matches(999, 100, false)); // clock rollback cannot create identity
        phase.record(1003, 120, 121, true);
        check(phase.matches(1003, 119, true));
        GpuiLocation.CallbackCounts counts = new GpuiLocation.CallbackCounts();
        check(counts.record(true, false, 110, 100, -1, true).equals("accepted"));
        check(counts.record(true, false, 99, 100, 110, true).equals("pre_request"));
        check(counts.record(true, false, 110, 100, 110, true).equals("duplicate"));
        check(counts.record(true, false, 105, 100, 110, true).equals("out_of_order"));
        check(counts.record(true, false, 120, 100, 110, false).equals("invalid_fix"));
        check(counts.record(true, true, 120, 100, 110, true).equals("paused"));
        check(counts.record(false, false, 120, 100, 110, true).equals("inactive"));
        check(counts.received == 7 && counts.accepted == 1 && counts.stale == 1
            && counts.duplicate == 1 && counts.outOfOrder == 1 && counts.invalid == 1
            && counts.paused == 1 && counts.inactive == 1);
        check(new GpuiLocation.CallbackCounts().received == 0); // independent requests
        check(java.util.Arrays.equals(GpuiLocation.ProviderPolicy.candidates(true, true, true),
            new String[]{"fused", "gps", "network"}));
        check(java.util.Arrays.equals(GpuiLocation.ProviderPolicy.candidates(false, true, true),
            new String[]{"gps", "network"}));
        check(java.util.Arrays.equals(GpuiLocation.ProviderPolicy.candidates(true, false, true),
            new String[]{"fused", "network"}));
        check(java.util.Arrays.equals(GpuiLocation.ProviderPolicy.candidates(false, true, false),
            new String[]{"network", "gps"}));
        check(GpuiLocation.ProviderPolicy.deliver(true, false, 101, 100, -1));
        check(!GpuiLocation.ProviderPolicy.deliver(true, false, 99, 100, -1)); // cache cannot become live
        check(!GpuiLocation.ProviderPolicy.deliver(false, false, 101, 100, -1)); // cancellation
        check(!GpuiLocation.ProviderPolicy.deliver(true, true, 101, 100, -1)); // pause
        check(!GpuiLocation.ProviderPolicy.deliver(true, false, 101, 100, 101)); // duplicate
        check(!GpuiLocation.ProviderPolicy.deliver(true, false, 100, 100, 101)); // out of order
        Object owner = new Object();
        check(!GpuiLocation.HostState.resumed(owner)); // first request before any confirmed resume
        GpuiLocation.HostState.set(owner, false); // pause before first subscription
        check(!GpuiLocation.HostState.resumed(owner));
        GpuiLocation.HostState.set(owner, true);
        check(GpuiLocation.HostState.resumed(owner));
        Runnable postedStart = () -> check(!GpuiLocation.HostState.resumed(owner));
        GpuiLocation.HostState.set(owner, false); // pause after posting, before MAIN runs
        postedStart.run();
        GpuiLocation.HostState.set(owner, true);
        check(GpuiLocation.HostState.resumed(owner));
        check(!GpuiLocation.HostState.resumed(new Object())); // no cross-Activity state
        check(GpuiLocation.Fix.valid(37.5, 127, true, 5, 1700000000001L));
        check(GpuiLocation.Fix.valid(-90, 180, true, 0, 0));
        check(!GpuiLocation.Fix.valid(91, 127, true, 5, 1));
        check(!GpuiLocation.Fix.valid(37, -181, true, 5, 1));
        check(!GpuiLocation.Fix.valid(Double.NaN, 127, true, 5, 1));
        check(!GpuiLocation.Fix.valid(37, Double.POSITIVE_INFINITY, true, 5, 1));
        check(!GpuiLocation.Fix.valid(37, 127, false, 0, 1));
        check(!GpuiLocation.Fix.valid(37, 127, true, Float.NaN, 1));
        check(!GpuiLocation.Fix.valid(37, 127, true, -1, 1));
        check(!GpuiLocation.Fix.valid(37, 127, true, 5, -1));
        System.out.println("PASS Android production location fix validation (10 cases)");
    }
    private static void check(boolean value) { if (!value) throw new AssertionError(); }
}
