package dev.gpui.mobile;

import android.app.Activity;
import android.content.ContentResolver;
import android.content.ContentValues;
import android.database.Cursor;
import android.net.Uri;
import android.provider.CalendarContract;

import java.util.TimeZone;

public final class GpuiCalendar {

    public static String getCalendars(Activity activity) {
        ContentResolver cr = activity.getContentResolver();
        StringBuilder result = new StringBuilder();

        Cursor cursor = null;
        try {
            cursor = cr.query(
                CalendarContract.Calendars.CONTENT_URI,
                new String[]{
                    CalendarContract.Calendars._ID,
                    CalendarContract.Calendars.CALENDAR_DISPLAY_NAME,
                    CalendarContract.Calendars.CALENDAR_ACCESS_LEVEL,
                    CalendarContract.Calendars.CALENDAR_COLOR,
                },
                null, null, null
            );

            if (cursor == null) throw new IllegalStateException("Calendar provider returned no cursor");
            while (cursor.moveToNext()) {
                if (result.length() > 0) result.append("\n");
                String id = cursor.getString(0);
                String name = cursor.getString(1);
                int access = cursor.getInt(2);
                int color = cursor.getInt(3);
                boolean readOnly = access < CalendarContract.Calendars.CAL_ACCESS_CONTRIBUTOR;
                result.append(id).append("|")
                      .append(nullSafe(name)).append("|")
                      .append(readOnly ? "1" : "0").append("|")
                      .append(color & 0xFFFFFFFFL);
            }
        } catch (Exception e) {
            throw new IllegalStateException("getCalendars failed", e);
        } finally {
            if (cursor != null) cursor.close();
        }
        return result.toString();
    }

    public static String getEvents(Activity activity, String calendarId, long startMs, long endMs) {
        ContentResolver cr = activity.getContentResolver();
        StringBuilder result = new StringBuilder();

        String selection = CalendarContract.Instances.CALENDAR_ID + " = ? AND " +
            CalendarContract.Instances.END + " > ? AND " +
            CalendarContract.Instances.BEGIN + " < ?";
        Uri.Builder range = CalendarContract.Instances.CONTENT_URI.buildUpon();
        android.content.ContentUris.appendId(range, startMs);
        android.content.ContentUris.appendId(range, endMs);

        Cursor cursor = null;
        try {
            cursor = cr.query(
                range.build(),
                new String[]{
                    CalendarContract.Instances.EVENT_ID,
                    CalendarContract.Events.TITLE,
                    CalendarContract.Events.DESCRIPTION,
                    CalendarContract.Events.EVENT_LOCATION,
                    CalendarContract.Instances.BEGIN,
                    CalendarContract.Instances.END,
                    CalendarContract.Events.ALL_DAY,
                    CalendarContract.Events.CALENDAR_ID,
                },
                selection,
                new String[]{calendarId, String.valueOf(startMs), String.valueOf(endMs)},
                CalendarContract.Instances.BEGIN + " ASC"
            );

            if (cursor == null) throw new IllegalStateException("Calendar provider returned no cursor");
            while (cursor.moveToNext()) {
                if (result.length() > 0) result.append("\n");
                result.append(cursor.getString(0)).append("|")  // id
                      .append(nullSafe(cursor.getString(1))).append("|")  // title
                      .append(nullSafe(cursor.getString(2))).append("|")  // description
                      .append(nullSafe(cursor.getString(3))).append("|")  // location
                      .append(cursor.getLong(4)).append("|")   // startMs
                      .append(cursor.getLong(5)).append("|")   // endMs
                      .append(cursor.getInt(6) != 0 ? "1" : "0").append("|") // allDay
                      .append(cursor.getString(7));  // calendarId
            }
        } catch (Exception e) {
            throw new IllegalStateException("getEvents failed", e);
        } finally {
            if (cursor != null) cursor.close();
        }
        return result.toString();
    }

    public static String createEvent(Activity activity, String calendarId,
            String title, String description, String location,
            long startMs, long endMs, boolean allDay) {
        if (endMs <= startMs || (allDay && (startMs % 86400000L != 0 || endMs % 86400000L != 0)))
            throw new IllegalArgumentException("Invalid event range; all-day timestamps must be UTC midnight with exclusive end");
        ContentValues values = new ContentValues();
        values.put(CalendarContract.Events.CALENDAR_ID, Long.parseLong(calendarId));
        values.put(CalendarContract.Events.TITLE, title);
        values.put(CalendarContract.Events.DESCRIPTION, description);
        values.put(CalendarContract.Events.EVENT_LOCATION, location);
        values.put(CalendarContract.Events.DTSTART, startMs);
        values.put(CalendarContract.Events.DTEND, endMs);
        values.put(CalendarContract.Events.ALL_DAY, allDay ? 1 : 0);
        values.put(CalendarContract.Events.EVENT_TIMEZONE, allDay ? "UTC" : TimeZone.getDefault().getID());

        try {
            Uri uri = activity.getContentResolver().insert(CalendarContract.Events.CONTENT_URI, values);
            if (uri != null) {
                return uri.getLastPathSegment();
            }
        } catch (Exception e) {
            throw new IllegalStateException("createEvent failed", e);
        }
        throw new IllegalStateException("Calendar provider returned no created event URI");
    }

    public static boolean deleteEvent(Activity activity, String eventId) {
        // Validate before obtaining any resolver: an empty URI segment would
        // otherwise select the collection and risk deleting unrelated rows.
        long rowId = eventRowId(eventId);
        try {
            Uri uri = android.content.ContentUris.withAppendedId(CalendarContract.Events.CONTENT_URI, rowId);
            ContentResolver resolver = activity.getContentResolver();
            try (Cursor cursor = resolver.query(uri, new String[]{
                    CalendarContract.Events.RRULE, CalendarContract.Events.RDATE,
                    CalendarContract.Events.ORIGINAL_ID}, null, null, null)) {
                if (cursor == null) throw new IllegalStateException("Calendar provider returned no cursor");
                if (!cursor.moveToFirst()) return false;
                if (isRecurring(cursor.getString(0), cursor.getString(1), !cursor.isNull(2)))
                    throw new IllegalArgumentException("Recurring event deletion requires an explicit scope and is unsupported");
            }
            // Check again atomically with deletion: a concurrent provider edit
            // must not turn a checked standalone event into a deleted series.
            // CalendarProvider rejects selections on events/#. Use collection
            // routing with an exact bound ID AND the recurrence safety guard.
            int rows = resolver.delete(CalendarContract.Events.CONTENT_URI,
                DELETE_STANDALONE_SELECTION, new String[]{Long.toString(rowId)});
            return rows > 0;
        } catch (Exception e) {
            throw new IllegalStateException("deleteEvent failed", e);
        }
    }

    static final String DELETE_STANDALONE_SELECTION =
        "_id = ? AND (rrule IS NULL OR rrule = '') AND (rdate IS NULL OR rdate = '') AND original_id IS NULL";

    static long eventRowId(String value) {
        if (value == null || value.isEmpty()) throw new IllegalArgumentException("Event ID must be a positive Android row ID");
        for (int i = 0; i < value.length(); i++) {
            if (value.charAt(i) < '0' || value.charAt(i) > '9')
                throw new IllegalArgumentException("Event ID must contain only ASCII digits");
        }
        long id = Long.parseLong(value);
        if (id <= 0) throw new IllegalArgumentException("Event ID must be positive");
        return id;
    }

    static boolean isRecurring(String rule, String dates, boolean original) {
        return original || (rule != null && !rule.isEmpty()) || (dates != null && !dates.isEmpty());
    }

    private static String nullSafe(String s) {
        return s == null ? "" : s.replace("%", "%25").replace("|", "%7C")
            .replace("\n", "%0A").replace("\r", "%0D").replace(",", "%2C").replace(":", "%3A");
    }

    private GpuiCalendar() {}
}
