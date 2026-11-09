package dev.gpui.mobile;

/** Pure policy assertions against production helpers; no Android stand-ins. */
public final class GpuiCalendarContractTest {
    private static void check(boolean ok) { if (!ok) throw new AssertionError(); }
    public static void main(String[] args) {
        for (String invalid : new String[]{null, "", "0", "-1", "+1", " 1", "1/2", "1 OR 1=1", "9223372036854775808"}) {
            try {
                // Null Activity is intentional: invalid IDs MUST fail before
                // any resolver/URI call (including Android SDK stub methods).
                GpuiCalendar.deleteEvent(null, invalid);
                throw new AssertionError("Invalid ID accepted");
            } catch (IllegalArgumentException expected) { }
        }
        check(GpuiCalendar.eventRowId("00042") == 42);
        check(GpuiCalendar.eventRowId("9223372036854775807") == Long.MAX_VALUE);
        check(GpuiCalendar.DELETE_STANDALONE_SELECTION.equals("_id = ? AND (rrule IS NULL OR rrule = '') AND (rdate IS NULL OR rdate = '') AND original_id IS NULL"));
        check(!GpuiCalendar.isRecurring(null, null, false));
        check(!GpuiCalendar.isRecurring("", "", false));
        check(GpuiCalendar.isRecurring("FREQ=DAILY", null, false));
        check(GpuiCalendar.isRecurring(null, "20261016T090000Z", false));
        check(GpuiCalendar.isRecurring(null, null, true));
        System.out.println("PASS calendar recurring-deletion policy");
    }
}
