package dev.gpui.mobile;

import android.app.Activity;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;

/**
 * Java helper for clipboard access on Android.
 *
 * <p>All public methods are {@code static} and called from Rust via JNI.</p>
 */
public final class GpuiClipboard {

    /**
     * Copy text to the clipboard.
     *
     * @param activity The current Activity.
     * @param text     The text to copy.
     */
    public static void setText(Activity activity, String text) {
        ClipboardManager cm = (ClipboardManager) activity.getSystemService(Context.CLIPBOARD_SERVICE);
        if (cm == null) throw new IllegalStateException("Clipboard service unavailable");
        ClipData clip = ClipData.newPlainText("text", text);
        cm.setPrimaryClip(clip);
    }

    /**
     * Read text from the clipboard.
     *
     * @param activity The current Activity.
     * @return The clipboard text, or {@code null} if empty or not text.
     */
    public static String getText(Activity activity) {
        ClipboardManager cm = (ClipboardManager) activity.getSystemService(Context.CLIPBOARD_SERVICE);
        if (cm == null) throw new IllegalStateException("Clipboard service unavailable");
        if (!cm.hasPrimaryClip()) return null;
        ClipData clip = cm.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) return null;
        CharSequence text = clip.getItemAt(0).getText();
        return text != null ? text.toString() : null;
    }

    /**
     * Check if the clipboard has text content.
     *
     * @param activity The current Activity.
     * @return {@code true} if the clipboard contains text.
     */
    public static boolean hasText(Activity activity) {
        ClipboardManager cm = (ClipboardManager) activity.getSystemService(Context.CLIPBOARD_SERVICE);
        if (cm == null) throw new IllegalStateException("Clipboard service unavailable");
        if (!cm.hasPrimaryClip()) return false;
        ClipData clip = cm.getPrimaryClip();
        return clip != null && clip.getItemCount() > 0 && clip.getItemAt(0).getText() != null;
    }

    // Binary v1: count, then kind byte + length-prefixed UTF-8/data fields.
    // Text: text, nullable metadata. Image: MIME, encoded bytes. File: local path.
    // All integers are big-endian; no Java modified UTF-8, so NUL is preserved.
    private static byte[] field(java.io.DataInputStream in) throws java.io.IOException {
        int length = in.readInt();
        if (length < 0 || length > in.available()) throw new java.io.IOException("Invalid clipboard field length");
        byte[] bytes = new byte[length]; in.readFully(bytes); return bytes;
    }
    private static String string(java.io.DataInputStream in) throws java.io.IOException {
        return new String(field(in), java.nio.charset.StandardCharsets.UTF_8);
    }
    private static void field(java.io.DataOutputStream out, byte[] bytes) throws java.io.IOException {
        out.writeInt(bytes.length); out.write(bytes);
    }
    private static void string(java.io.DataOutputStream out, String text) throws java.io.IOException {
        field(out, text.getBytes(java.nio.charset.StandardCharsets.UTF_8));
    }
    private static ClipboardManager manager(Activity activity) {
        ClipboardManager manager = (ClipboardManager) activity.getSystemService(Context.CLIPBOARD_SERVICE);
        if (manager == null) throw new IllegalStateException("Clipboard service unavailable");
        return manager;
    }
    private static String extension(String mime) {
        switch (mime) {
            case "image/png": return "png"; case "image/jpeg": return "jpg";
            case "image/webp": return "webp"; case "image/gif": return "gif";
            case "image/svg+xml": return "svg"; case "image/bmp": return "bmp";
            case "image/tiff": return "tiff"; case "image/x-icon": return "ico";
            case "image/x-portable-anymap": return "pnm";
            default: throw new IllegalArgumentException("Unsupported clipboard image MIME: " + mime);
        }
    }
    private static String canonicalImageMime(String mime) {
        if (mime == null) return null;
        switch (mime) {
            case "image/jpg": case "image/pjpeg": return "image/jpeg";
            case "image/x-png": return "image/png";
            case "image/x-bmp": case "image/x-ms-bmp": return "image/bmp";
            case "image/vnd.microsoft.icon": return "image/x-icon";
            case "image/x-portable-pixmap": case "image/x-portable-graymap":
            case "image/x-portable-bitmap": return "image/x-portable-anymap";
            default: return mime;
        }
    }
    private static String mime(java.io.File file) {
        String name = file.getName(); int dot = name.lastIndexOf('.');
        String mime = dot < 0 ? null : android.webkit.MimeTypeMap.getSingleton()
            .getMimeTypeFromExtension(name.substring(dot + 1).toLowerCase(java.util.Locale.ROOT));
        return mime == null ? "application/octet-stream" : mime;
    }
    private static java.io.File directory(Activity activity, boolean publishing) throws java.io.IOException {
        java.io.File root = new java.io.File(publishing ? activity.getCacheDir() : activity.getFilesDir(),
            publishing ? "gpui-clipboard" : "gpui-clipboard-imports");
        java.io.File directory = new java.io.File(root, java.util.UUID.randomUUID().toString());
        if (!directory.mkdirs()) throw new java.io.IOException("Cannot create clipboard storage");
        return directory;
    }
    private static void discard(java.io.File file) {
        if (file.exists() && !file.delete()) android.util.Log.w("GpuiClipboard", "Cannot remove owned clipboard staging file");
        java.io.File parent = file.getParentFile();
        if (parent != null && parent.exists() && !parent.delete()) android.util.Log.w("GpuiClipboard", "Cannot remove owned clipboard staging directory");
    }
    private static void copy(java.io.InputStream in, java.io.OutputStream out) throws java.io.IOException {
        if (in == null) throw new java.io.IOException("Clipboard provider returned no stream");
        byte[] buffer = new byte[32768]; int size;
        while ((size = in.read(buffer)) != -1) out.write(buffer, 0, size);
    }
    private static android.net.Uri uri(Activity activity, java.io.File file) {
        return androidx.core.content.FileProvider.getUriForFile(activity,
            activity.getPackageName() + ".gpui.clipboard.files", file);
    }
    public static void write(Activity activity, byte[] encoded) {
        java.util.ArrayList<java.io.File> staged = new java.util.ArrayList<>();
        boolean published = false;
        try (java.io.DataInputStream in = new java.io.DataInputStream(new java.io.ByteArrayInputStream(encoded))) {
            int count = in.readInt();
            if (count < 0 || count > encoded.length) throw new java.io.IOException("Invalid clipboard entry count");
            java.util.ArrayList<ClipData.Item> items = new java.util.ArrayList<>();
            java.util.LinkedHashSet<String> mimes = new java.util.LinkedHashSet<>();
            android.os.PersistableBundle extras = new android.os.PersistableBundle();
            for (int index = 0; index < count; index++) {
                int kind = in.readUnsignedByte();
                extras.putInt("dev.gpui.kind." + index, kind);
                if (kind == 0) {
                    String text = string(in);
                    int hasMetadata = in.readUnsignedByte();
                    if (hasMetadata > 1) throw new java.io.IOException("Invalid clipboard metadata flag");
                    if (hasMetadata == 1) extras.putString("dev.gpui.metadata." + index, string(in));
                    items.add(new ClipData.Item(text)); mimes.add("text/plain");
                } else if (kind == 1 || kind == 2) {
                    String value = string(in);
                    java.io.File source = kind == 2 ? new java.io.File(value).getCanonicalFile() : null;
                    if (source != null && (!source.isFile() || !source.canRead())) throw new java.io.IOException("Clipboard path is not a readable file");
                    String mime = kind == 1 ? value : mime(source);
                    java.io.File output = new java.io.File(directory(activity, true), kind == 1 ? "image." + extension(mime) : source.getName());
                    staged.add(output);
                    try (java.io.OutputStream out = new java.io.FileOutputStream(output)) {
                        if (kind == 1) out.write(field(in));
                        else try (java.io.InputStream input = new java.io.FileInputStream(source)) { copy(input, out); }
                    }
                    extras.putString("dev.gpui.mime." + index, mime);
                    items.add(new ClipData.Item(uri(activity, output))); mimes.add(mime);
                } else throw new java.io.IOException("Unknown clipboard entry kind");
            }
            if (in.available() != 0) throw new java.io.IOException("Trailing clipboard bytes");
            ClipboardManager cm = manager(activity);
            if (items.isEmpty()) {
                if (android.os.Build.VERSION.SDK_INT >= 28) cm.clearPrimaryClip();
                else cm.setPrimaryClip(ClipData.newPlainText("", ""));
            } else {
                android.content.ClipDescription description = new android.content.ClipDescription("GPUI", mimes.toArray(new String[0]));
                description.setExtras(extras);
                ClipData clip = new ClipData(description, items.get(0));
                for (int i = 1; i < items.size(); i++) clip.addItem(items.get(i));
                cm.setPrimaryClip(clip);
            }
            published = true;
        } catch (java.io.IOException error) { throw new IllegalStateException("Clipboard publication failed", error); }
        finally { if (!published) for (java.io.File file : staged) discard(file); }
        // Successful staging belongs to the native clipboard's URI lifetime. Do not
        // delete an earlier publication while another app may still be reading it.
    }
    public static byte[] read(Activity activity) {
        return readClip(activity, manager(activity).getPrimaryClip());
    }
    static byte[] readClip(Activity activity, ClipData clip) {
        if (clip == null || clip.getItemCount() == 0) return null;
        android.os.PersistableBundle extras = clip.getDescription().getExtras();
        java.util.ArrayList<java.io.File> imports = new java.util.ArrayList<>();
        boolean complete = false;
        try {
            java.io.ByteArrayOutputStream bytes = new java.io.ByteArrayOutputStream();
            java.io.DataOutputStream out = new java.io.DataOutputStream(bytes);
            out.writeInt(clip.getItemCount());
            for (int index = 0; index < clip.getItemCount(); index++) {
                ClipData.Item item = clip.getItemAt(index);
                android.net.Uri uri = item.getUri();
                if (uri == null && item.getText() != null) {
                    out.writeByte(0); string(out, item.getText().toString());
                    String metadata = extras == null ? null : extras.getString("dev.gpui.metadata." + index);
                    out.writeByte(metadata == null ? 0 : 1); if (metadata != null) string(out, metadata);
                } else if (uri != null) {
                    String scheme = uri.getScheme();
                    requireContentScheme(scheme);
                    String mime = activity.getContentResolver().getType(uri);
                    // A per-entry type is necessary for mixed-MIME clips. Do not trust
                    // external metadata as file authority; resolver access still checks grants.
                    if (extras != null && extras.getString("dev.gpui.mime." + index) != null) mime = extras.getString("dev.gpui.mime." + index);
                    boolean image = mime != null && mime.startsWith("image/") && (extras == null || extras.getInt("dev.gpui.kind." + index, -1) != 2);
                    if (image) {
                        mime = canonicalImageMime(mime);
                        extension(mime); // Reject unsupported formats, never mislabel bytes.
                        java.io.ByteArrayOutputStream imageBytes = new java.io.ByteArrayOutputStream();
                        try (java.io.InputStream input = activity.getContentResolver().openInputStream(uri)) { copy(input, imageBytes); }
                        out.writeByte(1); string(out, mime); field(out, imageBytes.toByteArray());
                    } else {
                        String name = "clipboard-file";
                        if ("content".equals(scheme)) {
                            try (android.database.Cursor cursor = activity.getContentResolver().query(uri,
                                    new String[]{android.provider.OpenableColumns.DISPLAY_NAME}, null, null, null)) {
                                if (cursor != null && cursor.moveToFirst() && !cursor.isNull(0)) name = cursor.getString(0);
                            }
                        }
                        name = name.replaceAll("[/\\\\\\p{Cntrl}]", "_");
                        if (name.isEmpty() || name.equals(".") || name.equals("..")) name = "clipboard-file";
                        java.io.File output = new java.io.File(directory(activity, false), name); imports.add(output);
                        try (java.io.InputStream input = activity.getContentResolver().openInputStream(uri);
                             java.io.OutputStream target = new java.io.FileOutputStream(output)) { copy(input, target); }
                        out.writeByte(2); string(out, output.getAbsolutePath());
                    }
                } else throw new java.io.IOException("Clipboard item has no supported text or URI");
            }
            out.flush(); complete = true; return bytes.toByteArray();
        } catch (java.io.IOException error) { throw new IllegalStateException("Clipboard import failed", error); }
        finally { if (!complete) for (java.io.File file : imports) discard(file); }
    }
    static void requireContentScheme(String scheme) throws java.io.IOException {
        // Never resolve sender-selected file paths under the receiving app's identity.
        if (!"content".equals(scheme)) throw new java.io.IOException("Clipboard requires a granted content URI");
    }
    public static void openFile(Activity activity, String path) {
        java.io.File staged = null;
        try {
            java.io.File source = new java.io.File(path).getCanonicalFile();
            String canonical = source.getPath();
            boolean owned = canonical.startsWith(activity.getFilesDir().getCanonicalPath() + java.io.File.separator)
                || canonical.startsWith(activity.getCacheDir().getCanonicalPath() + java.io.File.separator);
            if (!owned || !source.isFile() || !source.canRead()) throw new java.io.IOException("Open requires a readable app-owned file");
            staged = new java.io.File(directory(activity, true), source.getName());
            try (java.io.InputStream input = new java.io.FileInputStream(source);
                 java.io.OutputStream output = new java.io.FileOutputStream(staged)) { copy(input, output); }
            android.net.Uri uri = uri(activity, staged);
            android.content.Intent intent = new android.content.Intent(android.content.Intent.ACTION_VIEW)
                .setDataAndType(uri, mime(source)).addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION);
            intent.setClipData(ClipData.newRawUri("file", uri));
            activity.startActivity(intent);
        } catch (java.io.IOException | RuntimeException error) {
            if (staged != null) discard(staged);
            throw new IllegalStateException("Cannot open file with native viewer", error);
        }
    }
    private GpuiClipboard() {}
}
