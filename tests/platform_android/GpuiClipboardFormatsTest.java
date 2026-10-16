package dev.gpui.mobile;

import java.io.*;
import java.lang.reflect.*;
import java.nio.charset.StandardCharsets;

/** Host logic checks only; actual clipboard/URI grants require a foreground native test. */
public final class GpuiClipboardFormatsTest {
    /** Native-only fixture: must be explicitly invoked with a foreground Activity.
     * Same-host FileProvider success is not proof of cross-app grant delivery. */
    public static void nativeUriBoundary(android.app.Activity activity) throws Exception {
        android.content.ClipboardManager manager = (android.content.ClipboardManager)
            activity.getSystemService(android.content.Context.CLIPBOARD_SERVICE);
        android.content.ClipData previous = manager.getPrimaryClip();
        File directory = new File(activity.getCacheDir(), "gpui-clipboard/uri-test-" + java.util.UUID.randomUUID());
        if (!directory.mkdirs()) throw new IOException("Cannot create fixture directory");
        File sentinel = new File(directory, "private-sentinel.txt");
        File imported = null;
        byte[] expected = "private fixture sentinel".getBytes(StandardCharsets.UTF_8);
        try {
            try (OutputStream output = new FileOutputStream(sentinel)) { output.write(expected); }
            android.content.ClipData hostile = new android.content.ClipData("negative", new String[]{"text/plain"},
                new android.content.ClipData.Item(android.net.Uri.fromFile(sentinel)));
            // Exercise the incoming path directly: publishing file:// ourselves
            // can be rejected by sender-side StrictMode before reaching it.
            try { GpuiClipboard.readClip(activity, hostile); throw new AssertionError("Private file URI accepted"); }
            catch (IllegalStateException rejection) {
                if (!(rejection.getCause() instanceof IOException)) throw rejection;
            }
            android.net.Uri uri = androidx.core.content.FileProvider.getUriForFile(activity,
                activity.getPackageName() + ".gpui.clipboard.files", sentinel);
            manager.setPrimaryClip(new android.content.ClipData("positive", new String[]{"text/plain"},
                new android.content.ClipData.Item(uri)));
            byte[] result = GpuiClipboard.read(activity);
            if (result == null) throw new AssertionError("Native clipboard unavailable");
            DataInputStream input = new DataInputStream(new ByteArrayInputStream(result));
            if (input.readInt() != 1 || input.readByte() != 2) throw new AssertionError("Expected file entry");
            int length = input.readInt();
            if (length < 0 || length > result.length) throw new AssertionError("Invalid path length");
            byte[] path = new byte[length]; input.readFully(path);
            imported = new File(new String(path, StandardCharsets.UTF_8));
            if (!java.util.Arrays.equals(expected, java.nio.file.Files.readAllBytes(imported.toPath())))
                throw new AssertionError("Content URI bytes changed");
        } finally {
            if (previous != null) manager.setPrimaryClip(previous);
            else if (android.os.Build.VERSION.SDK_INT >= 28) manager.clearPrimaryClip();
            else manager.setPrimaryClip(android.content.ClipData.newPlainText("", ""));
            if (imported != null) { File parent = imported.getParentFile(); imported.delete(); parent.delete(); }
            sentinel.delete(); directory.delete();
        }
    }
    public static void main(String[] args) throws Exception {
        GpuiClipboard.requireContentScheme("content");
        for (String scheme : new String[]{"file", "http", "android.resource", "", null}) {
            try { GpuiClipboard.requireContentScheme(scheme); throw new AssertionError("Unsafe scheme accepted: " + scheme); }
            catch (IOException expected) { }
        }
        Method extension = GpuiClipboard.class.getDeclaredMethod("extension", String.class);
        extension.setAccessible(true);
        String[][] formats = {{"image/png","png"},{"image/jpeg","jpg"},{"image/webp","webp"},
            {"image/gif","gif"},{"image/svg+xml","svg"},{"image/bmp","bmp"},
            {"image/tiff","tiff"},{"image/x-icon","ico"},{"image/x-portable-anymap","pnm"}};
        for (String[] pair : formats) {
            if (!pair[1].equals(extension.invoke(null, pair[0]))) throw new AssertionError(pair[0]);
        }
        try { extension.invoke(null, "image/unknown"); throw new AssertionError("Unsupported MIME accepted"); }
        catch (InvocationTargetException expected) {
            if (!(expected.getCause() instanceof IllegalArgumentException)) throw expected;
        }
        Method encode = GpuiClipboard.class.getDeclaredMethod("string", DataOutputStream.class, String.class);
        Method decode = GpuiClipboard.class.getDeclaredMethod("string", DataInputStream.class);
        encode.setAccessible(true); decode.setAccessible(true);
        for (String value : new String[]{"", "a\u0000한😀", "metadata\n\u0000"}) {
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            encode.invoke(null, new DataOutputStream(bytes), value);
            byte[] encoded = bytes.toByteArray();
            if (encoded.length != 4 + value.getBytes(StandardCharsets.UTF_8).length) throw new AssertionError("Modified UTF-8 used");
            if (!value.equals(decode.invoke(null, new DataInputStream(new ByteArrayInputStream(encoded))))) throw new AssertionError("UTF-8 mismatch");
            for (int end = 0; end < encoded.length; end++) {
                try { decode.invoke(null, new DataInputStream(new ByteArrayInputStream(java.util.Arrays.copyOf(encoded, end)))); throw new AssertionError("Truncation accepted"); }
                catch (InvocationTargetException expected) { if (!(expected.getCause() instanceof IOException)) throw expected; }
            }
        }
        System.out.println("Clipboard MIME/UTF-8/truncation logic PASS (not native clipboard verification)");
    }
}
