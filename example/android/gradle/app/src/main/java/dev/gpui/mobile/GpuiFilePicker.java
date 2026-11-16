package dev.gpui.mobile;

import android.app.Activity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.OpenableColumns;

import java.io.IOException;
import java.io.OutputStream;
import java.util.ArrayList;

/**
 * File picker helper using the Storage Access Framework.
 *
 * <p>All public methods are static and called from Rust via JNI.
 * They block the calling thread until the user completes or cancels the picker.</p>
 */
public final class GpuiFilePicker {

    /**
     * Open a single file picker.
     *
     * @param activity The current Activity.
     * @param mimeTypes Pipe-separated MIME types (e.g. "image/jpeg|image/png") or "*\/*" for all.
     * @return The selected file URI as a string, or null if cancelled.
     */
    public static String openFile(final Activity activity, final String mimeTypes) {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        applyMimeTypes(intent, mimeTypes);

        ArrayList<String> result = launchPicker(activity, intent);
        if (result != null && !result.isEmpty()) {
            return result.get(0);
        }
        return null;
    }

    /**
     * Open a multi-file picker.
     *
     * @param activity The current Activity.
     * @param mimeTypes Pipe-separated MIME types.
     * @return Array of selected file URIs, or null if cancelled.
     */
    public static String[] openFiles(final Activity activity, final String mimeTypes) {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
        applyMimeTypes(intent, mimeTypes);

        ArrayList<String> result = launchPicker(activity, intent);
        if (result != null) {
            return result.toArray(new String[0]);
        }
        return null;
    }

    /**
     * Open a save-file dialog (ACTION_CREATE_DOCUMENT).
     *
     * @param activity The current Activity.
     * @param mimeType MIME type for the file to create.
     * @param suggestedName Suggested file name.
     * @return The chosen save URI as a string, or null if cancelled.
     */
    public static String getSavePath(final Activity activity, final String mimeType, final String suggestedName) {
        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType(mimeType != null ? mimeType : "*/*");
        if (suggestedName != null) {
            intent.putExtra(Intent.EXTRA_TITLE, suggestedName);
        }

        ArrayList<String> result = launchPicker(activity, intent);
        if (result != null && !result.isEmpty()) {
            return result.get(0);
        }
        return null;
    }

    /**
     * Write existing contents to a document chosen through ACTION_CREATE_DOCUMENT.
     *
     * @param activity The current Activity.
     * @param mimeType MIME type for the file to create.
     * @param suggestedName Suggested file name.
     * @param contents Bytes to write. A failed write can leave a partial document.
     * @return The created document URI, or null if cancelled.
     */
    public static String exportFile(final Activity activity, final String mimeType,
            final String suggestedName, final byte[] contents) {
        Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType(mimeType != null ? mimeType : "*/*");
        intent.putExtra(Intent.EXTRA_TITLE, GpuiPickerFiles.safeName(suggestedName));

        ArrayList<String> result = launchPicker(activity, intent);
        if (result == null || result.isEmpty()) {
            return null;
        }
        Uri uri = Uri.parse(result.get(0));
        try (OutputStream output = activity.getContentResolver().openOutputStream(uri, "wt")) {
            if (output == null) {
                throw new IOException("Provider returned no output stream");
            }
            output.write(contents == null ? new byte[0] : contents);
        } catch (IOException | RuntimeException error) {
            // The provider may return an existing document; never delete the user's destination.
            throw new IllegalStateException(
                    "Cannot export document contents; the destination may contain a partial write",
                    error);
        }
        return uri.toString();
    }

    /**
     * Return the provider display name for a document URI, or null when the
     * provider does not expose OpenableColumns.DISPLAY_NAME.
     *
     * <p>Never substitutes the URI document ID for the display name.</p>
     */
    public static String displayName(final Activity activity, final String uriString) {
        if (uriString == null || uriString.isEmpty()) return null;
        try (Cursor cursor = activity.getContentResolver().query(
                Uri.parse(uriString),
                new String[]{OpenableColumns.DISPLAY_NAME},
                null, null, null)) {
            if (cursor == null || !cursor.moveToFirst()) return null;
            int column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
            if (column < 0) return null;
            String name = cursor.getString(column);
            return name == null || name.isEmpty() ? null : name;
        } catch (RuntimeException error) {
            return null;
        }
    }

    /**
     * Open a directory picker (ACTION_OPEN_DOCUMENT_TREE).
     *
     * @param activity The current Activity.
     * @return The chosen directory URI as a string, or null if cancelled.
     */
    public static String getDirectoryPath(final Activity activity) {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);

        ArrayList<String> result = launchPicker(activity, intent);
        if (result != null && !result.isEmpty()) {
            return result.get(0);
        }
        return null;
    }

    // ── Internal ─────────────────────────────────────────────────────────

    private static ArrayList<String> launchPicker(Activity activity, Intent intent) {
        return GpuiPickerActivity.launch(activity, intent,
                Intent.ACTION_OPEN_DOCUMENT.equals(intent.getAction()), null, GpuiPickerActivity.KIND_FILE);
    }

    private static void applyMimeTypes(Intent intent, String mimeTypes) {
        if (mimeTypes == null || mimeTypes.isEmpty() || mimeTypes.equals("*/*")) {
            intent.setType("*/*");
            return;
        }

        String[] types = mimeTypes.split("\\|");
        if (types.length == 1) {
            intent.setType(types[0]);
        } else {
            intent.setType("*/*");
            intent.putExtra(Intent.EXTRA_MIME_TYPES, types);
        }
    }

    private GpuiFilePicker() {}
}
