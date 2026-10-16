package dev.gpui.mobile;

import android.app.Activity;
import android.content.ContentValues;
import android.database.Cursor;
import android.net.Uri;
import android.provider.CalendarContract;

/** Explicit native harness only. Requires read/write calendar authorization and
 * an existing writable calendar ID. Creates and removes ONLY its own events.
 */
public final class GpuiCalendarNativeTest {
    public static void run(Activity activity, long calendarId) {
        Uri recurring = null;
        String standalone = null;
        String survivor = null;
        try {
            ContentValues values = new ContentValues();
            values.put(CalendarContract.Events.CALENDAR_ID, calendarId);
            values.put(CalendarContract.Events.TITLE, "GPUI owned recurrence safety fixture");
            values.put(CalendarContract.Events.DTSTART, 1792108800000L);
            values.put(CalendarContract.Events.DURATION, "PT1H");
            values.put(CalendarContract.Events.EVENT_TIMEZONE, "UTC");
            values.put(CalendarContract.Events.RRULE, "FREQ=DAILY;COUNT=3");
            recurring = activity.getContentResolver().insert(CalendarContract.Events.CONTENT_URI, values);
            if (recurring == null) throw new AssertionError("fixture insert failed");
            try {
                GpuiCalendar.deleteEvent(activity, recurring.getLastPathSegment());
                throw new AssertionError("recurring deletion must explicitly reject");
            } catch (IllegalStateException expected) {
                if (!(expected.getCause() instanceof IllegalArgumentException)) throw expected;
            }
            try (Cursor c = activity.getContentResolver().query(recurring, new String[]{CalendarContract.Events.RRULE}, null, null, null)) {
                if (c == null || !c.moveToFirst() || !"FREQ=DAILY;COUNT=3".equals(c.getString(0)))
                    throw new AssertionError("recurring event was changed or removed");
            }
            standalone = GpuiCalendar.createEvent(activity, Long.toString(calendarId), "GPUI owned day fixture", "", "", 0, 86400000, true);
            survivor = GpuiCalendar.createEvent(activity, Long.toString(calendarId), "GPUI owned survivor fixture", "", "", 86400000, 172800000, true);
            if (!GpuiCalendar.deleteEvent(activity, standalone)) throw new AssertionError("standalone not deleted");
            try (Cursor c = activity.getContentResolver().query(Uri.withAppendedPath(CalendarContract.Events.CONTENT_URI, survivor), new String[]{CalendarContract.Events._ID}, null, null, null)) {
                if (c == null || !c.moveToFirst()) throw new AssertionError("Unrelated event deleted");
            }
            if (GpuiCalendar.deleteEvent(activity, standalone)) throw new AssertionError("absent event reported deleted");
            standalone = null;
            try {
                GpuiCalendar.createEvent(activity, Long.toString(calendarId), "invalid", "", "", 1, 86400000, true);
                throw new AssertionError("non-midnight accepted");
            } catch (IllegalArgumentException expected) { }
        } finally {
            if (survivor != null) activity.getContentResolver().delete(Uri.withAppendedPath(CalendarContract.Events.CONTENT_URI, survivor), null, null);
            if (standalone != null) activity.getContentResolver().delete(Uri.withAppendedPath(CalendarContract.Events.CONTENT_URI, standalone), null, null);
            // Intentional direct provider deletion ONLY of the fixture-owned series.
            if (recurring != null) activity.getContentResolver().delete(recurring, null, null);
        }
    }
}
