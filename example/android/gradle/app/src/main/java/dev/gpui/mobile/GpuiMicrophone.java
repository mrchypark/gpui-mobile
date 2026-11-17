package dev.gpui.mobile;

import android.app.Activity;
import android.content.ContentUris;
import android.database.Cursor;
import android.media.MediaRecorder;
import android.net.Uri;
import android.os.Build;
import android.provider.MediaStore;
import android.util.Log;

import java.io.File;
import java.io.IOException;
import java.util.ArrayList;
import java.util.UUID;

public class GpuiMicrophone {
    private static final String TAG = "GpuiMicrophone";
    private static MediaRecorder sRecorder;
    private static GpuiWavRecorder sWav;
    private static String sOutputPath;
    private static long sStartTime;
    private static long sRecordedTime;
    private static boolean sIsRecording;
    private static boolean sIsPaused;
    private static volatile long sGeneration;
    private static RuntimeException sFailure;
    private static android.app.Application sApplication;
    private static android.app.Application.ActivityLifecycleCallbacks sLifecycle;

    public static synchronized boolean isAvailable(Activity activity) {
        return activity != null && activity.getPackageManager().hasSystemFeature("android.hardware.microphone");
    }

    private static native void nativeOnPcmFrames(float[] frames);

    static void emitPcmFrames(float[] frames) {
        if (frames == null || frames.length == 0) return;
        try {
            nativeOnPcmFrames(frames);
        } catch (UnsatisfiedLinkError ignored) {
        }
    }

    public static synchronized String startRecordingStream(Activity activity, int sampleRate, int channels) {
        if (sIsRecording || activity == null) return null;
        if (activity.isFinishing() || activity.isDestroyed()) return null;
        if (sampleRate <= 0 || (channels != 1 && channels != 2)) return null;
        try {
            sFailure = null;
            sOutputPath = new File(activity.getCacheDir(), "recording_" + UUID.randomUUID() + ".wav").getAbsolutePath();
            sWav = new GpuiWavRecorder(sOutputPath, sampleRate, channels, true);
            sIsRecording = true;
            sIsPaused = false;
            observeOwner(activity);
            return sOutputPath;
        } catch (Exception e) {
            Log.e(TAG, "Failed to start streaming recording", e);
            cleanup();
            return null;
        }
    }

    public static String[] queryAudioEntries(Activity activity, long sinceMs) {
        if (activity == null) throw new IllegalStateException("Activity is unavailable");
        Uri collection = MediaStore.Audio.Media.EXTERNAL_CONTENT_URI;
        String[] projection = new String[] {
            MediaStore.Audio.Media._ID,
            MediaStore.Audio.Media.DISPLAY_NAME,
            MediaStore.Audio.Media.DURATION,
            MediaStore.Audio.Media.DATE_ADDED,
            MediaStore.Audio.Media.SIZE,
            MediaStore.Audio.Media.MIME_TYPE,
        };
        String selection = sinceMs > 0 ? MediaStore.Audio.Media.DATE_ADDED + " >= ?" : null;
        String[] selectionArgs = sinceMs > 0 ? new String[] { Long.toString(sinceMs / 1000L) } : null;
        String sortOrder = MediaStore.Audio.Media.DATE_ADDED + " DESC";
        ArrayList<String> out = new ArrayList<>();
        try (Cursor cursor = activity.getContentResolver().query(collection, projection, selection, selectionArgs, sortOrder)) {
            if (cursor == null) return new String[0];
            int idCol = cursor.getColumnIndex(MediaStore.Audio.Media._ID);
            int nameCol = cursor.getColumnIndex(MediaStore.Audio.Media.DISPLAY_NAME);
            int durCol = cursor.getColumnIndex(MediaStore.Audio.Media.DURATION);
            int addedCol = cursor.getColumnIndex(MediaStore.Audio.Media.DATE_ADDED);
            int sizeCol = cursor.getColumnIndex(MediaStore.Audio.Media.SIZE);
            int mimeCol = cursor.getColumnIndex(MediaStore.Audio.Media.MIME_TYPE);
            while (cursor.moveToNext()) {
                if (idCol < 0) continue;
                long id = cursor.getLong(idCol);
                Uri itemUri = ContentUris.withAppendedId(collection, id);
                String name = (nameCol >= 0 && !cursor.isNull(nameCol)) ? cursor.getString(nameCol) : "";
                String dur = (durCol >= 0 && !cursor.isNull(durCol)) ? Long.toString(cursor.getLong(durCol)) : "";
                String added = (addedCol >= 0 && !cursor.isNull(addedCol)) ? Long.toString(cursor.getLong(addedCol) * 1000L) : "";
                String size = (sizeCol >= 0 && !cursor.isNull(sizeCol)) ? Long.toString(cursor.getLong(sizeCol)) : "";
                String mime = (mimeCol >= 0 && !cursor.isNull(mimeCol)) ? cursor.getString(mimeCol) : "";
                out.add(itemUri.toString());
                out.add(name != null ? name : "");
                out.add(dur);
                out.add(added);
                out.add(size);
                out.add(mime != null ? mime : "");
            }
        }
        return out.toArray(new String[0]);
    }

    public static synchronized String startRecording(Activity activity, int format, int sampleRate, int channels, int bitRate) {
        if (sIsRecording) return null;
        if (activity == null) return null;
        if (activity.isFinishing() || activity.isDestroyed()) return null;
        if (sampleRate <= 0 || (channels != 1 && channels != 2) || (format != 1 && bitRate <= 0)) return null;

        try {
            sFailure = null;
            String ext;
            int outputFormat;
            int audioEncoder;

            switch (format) {
                case 1: // WAV uses actual PCM, never a differently encoded .wav file.
                    sOutputPath = new File(activity.getCacheDir(), "recording_" + UUID.randomUUID() + ".wav").getAbsolutePath();
                    sWav = new GpuiWavRecorder(sOutputPath, sampleRate, channels);
                    sIsRecording = true;
                    sIsPaused = false;
                    observeOwner(activity);
                    return sOutputPath;
                case 2: // AMR
                    ext = "amr";
                    outputFormat = MediaRecorder.OutputFormat.AMR_NB;
                    audioEncoder = MediaRecorder.AudioEncoder.AMR_NB;
                    break;
                default: // AAC
                    ext = "m4a";
                    outputFormat = MediaRecorder.OutputFormat.MPEG_4;
                    audioEncoder = MediaRecorder.AudioEncoder.AAC;
                    break;
            }

            String fileName = "recording_" + UUID.randomUUID().toString() + "." + ext;
            File outputFile = new File(activity.getCacheDir(), fileName);
            sOutputPath = outputFile.getAbsolutePath();

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                sRecorder = new MediaRecorder(activity);
            } else {
                sRecorder = new MediaRecorder();
            }

            sRecorder.setAudioSource(MediaRecorder.AudioSource.MIC);
            MediaRecorder recorder = sRecorder;
            sRecorder.setOnErrorListener((failed, what, extra) -> {
                synchronized (GpuiMicrophone.class) {
                    if (sRecorder != recorder) return;
                    sFailure = new IllegalStateException("Recorder error " + what + "/" + extra);
                    cleanup();
                }
            });
            sRecorder.setOutputFormat(outputFormat);
            sRecorder.setAudioEncoder(audioEncoder);
            sRecorder.setAudioSamplingRate(sampleRate);
            sRecorder.setAudioChannels(channels);
            sRecorder.setAudioEncodingBitRate(bitRate);
            sRecorder.setOutputFile(sOutputPath);

            sRecorder.prepare();
            sRecorder.start();
            sStartTime = android.os.SystemClock.elapsedRealtime();
            sRecordedTime = 0;
            sIsRecording = true;
            sIsPaused = false;
            observeOwner(activity);

            return sOutputPath;
        } catch (Exception e) {
            Log.e(TAG, "Failed to start recording", e);
            cleanup();
            return null;
        }
    }

    public static synchronized String stopRecording() {
        if (sFailure != null) throw sFailure;
        if (!sIsRecording) return null;

        try {
            long duration;
            if (sWav != null) {
                GpuiWavRecorder wav = sWav;
                sWav = null;
                duration = wav.stop();
            } else {
                sRecorder.stop();
                duration = sRecordedTime + (sIsPaused ? 0 : android.os.SystemClock.elapsedRealtime() - sStartTime);
            }
            String result = sOutputPath + "|" + duration;
            cleanup();
            return result;
        } catch (Exception e) {
            Log.e(TAG, "Failed to stop recording", e);
            cleanup();
            throw new IllegalStateException("Recording failed", e);
        }
    }

    public static synchronized boolean isRecording() {
        if (sWav != null) return sWav.isRecording();
        return sIsRecording && !sIsPaused;
    }

    public static synchronized boolean pauseRecording() {
        if (!sIsRecording || sIsPaused) return false;
        if (sWav != null) { sWav.pause(); sIsPaused = true; return true; }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            try {
                sRecorder.pause();
                sRecordedTime += android.os.SystemClock.elapsedRealtime() - sStartTime;
                sIsPaused = true;
                return true;
            } catch (Exception e) {
                Log.e(TAG, "Failed to pause recording", e);
                return false;
            }
        }
        return false;
    }

    public static synchronized boolean resumeRecording() {
        if (!sIsRecording || !sIsPaused) return false;
        if (sWav != null) { sWav.resume(); sIsPaused = false; return true; }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            try {
                sRecorder.resume();
                sStartTime = android.os.SystemClock.elapsedRealtime();
                sIsPaused = false;
                return true;
            } catch (Exception e) {
                Log.e(TAG, "Failed to resume recording", e);
                return false;
            }
        }
        return false;
    }

    public static synchronized double getAmplitude() {
        if (sFailure != null) throw sFailure;
        if (sWav != null) return sWav.amplitude();
        if (!sIsRecording || sRecorder == null || sIsPaused) return 0.0;
        try {
            int maxAmplitude = sRecorder.getMaxAmplitude();
            if (maxAmplitude <= 0) return 0.0;
            // Normalize: max amplitude for MediaRecorder is ~32767
            return Math.min(1.0, (double) maxAmplitude / 32767.0);
        } catch (Exception e) {
            return 0.0;
        }
    }

    private static void cleanup() {
        sGeneration++;
        if (sLifecycle != null) {
            sApplication.unregisterActivityLifecycleCallbacks(sLifecycle);
            sLifecycle = null;
            sApplication = null;
        }
        if (sWav != null) {
            GpuiWavRecorder wav = sWav;
            sWav = null;
            try { wav.stop(); }
            catch (IOException error) { sFailure = new IllegalStateException("PCM recording failed", error); }
        }
        if (sRecorder != null) {
            try {
                sRecorder.release();
            } catch (Exception e) {
                // ignore
            }
            sRecorder = null;
        }
        sIsRecording = false;
        sIsPaused = false;
    }

    private static void observeOwner(Activity owner) {
        final long generation = ++sGeneration;
        java.lang.ref.WeakReference<Activity> weakOwner = new java.lang.ref.WeakReference<>(owner);
        sApplication = owner.getApplication();
        sLifecycle = new android.app.Application.ActivityLifecycleCallbacks() {
            private void ended(Activity activity) {
                if (activity != weakOwner.get() || generation != sGeneration) return;
                new Thread(() -> {
                    synchronized (GpuiMicrophone.class) {
                        if (generation != sGeneration) return;
                        try { stopRecording(); }
                        catch (RuntimeException error) { sFailure = error; cleanup(); }
                    }
                }, "gpui-recording-cleanup").start();
            }
            public void onActivityStopped(Activity a) { ended(a); }
            public void onActivityDestroyed(Activity a) { ended(a); }
            public void onActivityCreated(Activity a, android.os.Bundle b) {}
            public void onActivityStarted(Activity a) {}
            public void onActivityResumed(Activity a) {}
            public void onActivityPaused(Activity a) {}
            public void onActivitySaveInstanceState(Activity a, android.os.Bundle b) {}
        };
        sApplication.registerActivityLifecycleCallbacks(sLifecycle);
    }
}
