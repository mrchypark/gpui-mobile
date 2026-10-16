package dev.gpui.mobile;

import android.app.Activity;
import android.content.ClipData;
import android.content.Intent;
import android.net.Uri;
import android.provider.MediaStore;
import androidx.core.content.FileProvider;
import java.io.File;
import java.io.IOException;
import java.util.ArrayList;

public final class GpuiImagePicker {
    public static String pickImage(Activity activity, int source, int cameraFacing,
            double maxWidth, double maxHeight, int quality) {
        validate(maxWidth, maxHeight, quality);
        ArrayList<String> files = select(activity, source, cameraFacing, false, false);
        if (files == null || files.isEmpty()) return null;
        File input = new File(files.get(0));
        try { return GpuiPickerFiles.transform(input, maxWidth, maxHeight, quality).getAbsolutePath(); }
        catch (RuntimeException error) { discard(input, error); throw error; }
    }

    public static String[] pickMultiImage(Activity activity, double maxWidth, double maxHeight, int quality) {
        validate(maxWidth, maxHeight, quality);
        ArrayList<String> files = select(activity, 0, 0, false, true);
        if (files == null) return null;
        ArrayList<String> outputs = new ArrayList<>();
        try {
            for (String path : files) outputs.add(GpuiPickerFiles.transform(new File(path), maxWidth, maxHeight, quality).getAbsolutePath());
            return outputs.toArray(new String[0]);
        } catch (RuntimeException error) {
            for (String path : outputs) discard(new File(path), error);
            for (String path : files) discard(new File(path), error);
            throw error;
        }
    }

    public static String pickVideo(Activity activity, int source, int cameraFacing) {
        ArrayList<String> files = select(activity, source, cameraFacing, true, false);
        return files == null || files.isEmpty() ? null : files.get(0);
    }

    private static void validate(double width, double height, int quality) {
        GpuiPickerFiles.targetSize(1, 1, width, height);
        if (quality < -1 || quality > 100) throw new IllegalArgumentException("JPEG quality must be 0..100");
    }

    private static ArrayList<String> select(Activity activity, int source, int cameraFacing, boolean video, boolean multiple) {
        Intent intent;
        File capture = null;
        try {
            if (source == 1) {
                intent = new Intent(video ? MediaStore.ACTION_VIDEO_CAPTURE : MediaStore.ACTION_IMAGE_CAPTURE);
                File directory = new File(activity.getFilesDir(), "gpui-picker-capture");
                if (!directory.isDirectory() && !directory.mkdirs()) throw new IOException("Cannot create capture directory");
                capture = File.createTempFile("capture-", video ? ".mp4" : ".jpg", directory);
                Uri output = FileProvider.getUriForFile(activity, activity.getPackageName() + ".gpui.picker.files", capture);
                intent.putExtra(MediaStore.EXTRA_OUTPUT, output);
                intent.setClipData(ClipData.newRawUri("camera output", output));
                intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
                if (cameraFacing == 1) {
                    intent.putExtra("android.intent.extras.CAMERA_FACING", 1);
                    intent.putExtra("android.intent.extras.LENS_FACING_FRONT", 1);
                    intent.putExtra("android.intent.extra.USE_FRONT_CAMERA", true);
                }
            } else if (source == 0) {
                intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
                intent.addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType(video ? "video/*" : "image/*");
                intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple);
                intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            } else {
                throw new IllegalArgumentException("Unknown image source");
            }
            return GpuiPickerActivity.launch(activity, intent, true, capture);
        } catch (IOException error) {
            throw new IllegalStateException("Cannot prepare camera output", error);
        } finally {
            // launch waits through interruption until native ownership is released.
            if (capture != null && capture.exists() && !capture.delete()) {
                throw new IllegalStateException("Cannot remove camera output: " + capture);
            }
        }
    }

    private static void discard(File file, RuntimeException error) {
        try { GpuiPickerFiles.discard(file); }
        catch (RuntimeException cleanup) { error.addSuppressed(cleanup); }
    }
    private GpuiImagePicker() {}
}
