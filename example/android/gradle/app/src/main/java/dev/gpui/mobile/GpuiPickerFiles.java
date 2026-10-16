package dev.gpui.mobile;

import android.content.Context;
import android.database.Cursor;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.ImageDecoder;
import android.graphics.Matrix;
import android.media.ExifInterface;
import android.net.Uri;
import android.os.Build;
import android.provider.OpenableColumns;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.UUID;

/** Private imports: returned files remain readable after the URI grant expires. */
final class GpuiPickerFiles {
    static String safeName(String name) {
        if (name == null || name.isEmpty() || name.equals(".") || name.equals("..")) return "selected";
        return name.replaceAll("[/\\\\\\p{Cntrl}]", "_");
    }

    static File importUri(Context context, Uri uri) {
        String name = null;
        if ("content".equals(uri.getScheme())) {
            try (Cursor cursor = context.getContentResolver().query(uri,
                    new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
                if (cursor != null && cursor.moveToFirst()) {
                    int column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                    if (column >= 0) name = cursor.getString(column);
                }
            }
        } else if ("file".equals(uri.getScheme())) {
            name = new File(uri.getPath()).getName();
        } else {
            throw new IllegalArgumentException("Unsupported picker URI scheme");
        }
        File directory = new File(context.getFilesDir(), "gpui-picker-imports/" + UUID.randomUUID());
        if (!directory.mkdirs()) throw new IllegalStateException("Cannot create private import directory");
        File output = new File(directory, safeName(name));
        try (InputStream input = context.getContentResolver().openInputStream(uri);
             FileOutputStream target = new FileOutputStream(output)) {
            if (input == null) throw new IOException("Provider returned no input stream");
            byte[] buffer = new byte[32768];
            int count;
            while ((count = input.read(buffer)) != -1) target.write(buffer, 0, count);
            return output;
        } catch (IOException | RuntimeException error) {
            discard(output);
            throw new IllegalStateException("Cannot import selected file", error);
        }
    }

    static void discard(File file) {
        if (file.exists() && !file.delete()) throw new IllegalStateException("Cannot remove partial import: " + file);
        File directory = file.getParentFile();
        if (directory != null && directory.exists() && !directory.delete()) {
            throw new IllegalStateException("Cannot remove import directory: " + directory);
        }
    }

    static int[] targetSize(int width, int height, double maxWidth, double maxHeight) {
        if (width <= 0 || height <= 0 || !Double.isFinite(maxWidth) || !Double.isFinite(maxHeight)
                || (maxWidth != 0 && maxWidth < 1) || (maxHeight != 0 && maxHeight < 1)) {
            throw new IllegalArgumentException("Invalid image dimensions");
        }
        double scale = Math.min(1.0, Math.min(maxWidth == 0 ? 1.0 : maxWidth / width,
                maxHeight == 0 ? 1.0 : maxHeight / height));
        return new int[]{Math.max(1, (int)Math.floor(width * scale)), Math.max(1, (int)Math.floor(height * scale))};
    }

    static File transform(File input, double maxWidth, double maxHeight, int quality) {
        targetSize(1, 1, maxWidth, maxHeight);
        if (quality < -1 || quality > 100) throw new IllegalArgumentException("JPEG quality must be 0..100");
        if (maxWidth == 0 && maxHeight == 0 && quality == -1) return input;
        Bitmap bitmap = null;
        File output = null;
        try {
            BitmapFactory.Options bounds = new BitmapFactory.Options();
            bounds.inJustDecodeBounds = true;
            BitmapFactory.decodeFile(input.getAbsolutePath(), bounds);
            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) throw new IOException("Unsupported image data");
            boolean jpeg = "image/jpeg".equals(bounds.outMimeType);
            // Quality is a JPEG-only option; preserve other formats if no resize is requested.
            if (maxWidth == 0 && maxHeight == 0 && !jpeg) return input;
            if (Build.VERSION.SDK_INT >= 28) {
                bitmap = ImageDecoder.decodeBitmap(ImageDecoder.createSource(input), (decoder, info, source) -> {
                    int[] size = targetSize(info.getSize().getWidth(), info.getSize().getHeight(), maxWidth, maxHeight);
                    decoder.setTargetSize(size[0], size[1]);
                    decoder.setAllocator(ImageDecoder.ALLOCATOR_SOFTWARE);
                });
            } else {
                int orientation = new ExifInterface(input.getAbsolutePath()).getAttributeInt(ExifInterface.TAG_ORIENTATION, 1);
                boolean swap = orientation >= 5 && orientation <= 8;
                int[] size = targetSize(swap ? bounds.outHeight : bounds.outWidth,
                        swap ? bounds.outWidth : bounds.outHeight, maxWidth, maxHeight);
                BitmapFactory.Options decode = new BitmapFactory.Options();
                int requiredWidth = swap ? size[1] : size[0];
                int requiredHeight = swap ? size[0] : size[1];
                decode.inSampleSize = 1;
                while (bounds.outWidth / (decode.inSampleSize * 2) >= requiredWidth
                        && bounds.outHeight / (decode.inSampleSize * 2) >= requiredHeight) decode.inSampleSize *= 2;
                bitmap = BitmapFactory.decodeFile(input.getAbsolutePath(), decode);
                if (bitmap == null) throw new IOException("Image decode failed");
                Matrix matrix = new Matrix();
                switch (orientation) {
                    case 2: matrix.setScale(-1, 1); break;
                    case 3: matrix.setRotate(180); break;
                    case 4: matrix.setScale(1, -1); break;
                    case 5: matrix.setRotate(90); matrix.postScale(-1, 1); break;
                    case 6: matrix.setRotate(90); break;
                    case 7: matrix.setRotate(-90); matrix.postScale(-1, 1); break;
                    case 8: matrix.setRotate(-90); break;
                    default: break;
                }
                Bitmap oriented = Bitmap.createBitmap(bitmap, 0, 0, bitmap.getWidth(), bitmap.getHeight(), matrix, true);
                if (oriented != bitmap) { bitmap.recycle(); bitmap = oriented; }
                Bitmap resized = Bitmap.createScaledBitmap(bitmap, size[0], size[1], true);
                if (resized != bitmap) { bitmap.recycle(); bitmap = resized; }
            }
            String stem = input.getName();
            int dot = stem.lastIndexOf('.');
            if (dot > 0) stem = stem.substring(0, dot);
            File directory = new File(input.getParentFile().getParentFile(), UUID.randomUUID().toString());
            if (!directory.mkdir()) throw new IOException("Cannot create transformed image directory");
            output = new File(directory, stem + (jpeg ? ".jpg" : ".png"));
            try (FileOutputStream stream = new FileOutputStream(output)) {
                if (!bitmap.compress(jpeg ? Bitmap.CompressFormat.JPEG : Bitmap.CompressFormat.PNG,
                        quality < 0 ? 100 : quality, stream)) throw new IOException("Image encode failed");
            }
            discard(input);
            return output;
        } catch (IOException | RuntimeException error) {
            if (output != null) discard(output);
            throw new IllegalStateException("Image transformation failed", error);
        } finally {
            if (bitmap != null) bitmap.recycle();
        }
    }
    private GpuiPickerFiles() {}
}
