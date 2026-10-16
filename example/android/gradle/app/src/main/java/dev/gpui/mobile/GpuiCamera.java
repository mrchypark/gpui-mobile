package dev.gpui.mobile;

import android.Manifest;
import android.app.Activity;
import android.content.Context;
import android.graphics.ImageFormat;
import android.graphics.SurfaceTexture;
import android.hardware.camera2.*;
import android.hardware.camera2.params.StreamConfigurationMap;
import android.media.Image;
import android.media.ImageReader;
import android.media.MediaRecorder;
import android.os.Environment;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.util.Size;
import android.util.SparseArray;
import android.view.Surface;
import android.view.TextureView;
import android.view.ViewGroup;
import android.widget.FrameLayout;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.UUID;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Semaphore;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Camera2 API helper for the GPUI camera package.
 *
 * <p>All public methods are static and called from Rust via JNI.</p>
 */
public final class GpuiCamera {

    private static final String TAG = "GpuiCamera";

    /** Resolution preset constants matching Rust's ResolutionPreset enum. */
    private static final int RES_LOW = 0;
    private static final int RES_MEDIUM = 1;
    private static final int RES_HIGH = 2;
    private static final int RES_VERY_HIGH = 3;
    private static final int RES_ULTRA_HIGH = 4;
    private static final int RES_MAX = 5;

    /** Flash mode constants matching Rust's FlashMode enum. */
    private static final int FLASH_OFF = 0;
    private static final int FLASH_AUTO = 1;
    private static final int FLASH_ALWAYS = 2;
    private static final int FLASH_TORCH = 3;

    // ── Session management ───────────────────────────────────────────────

    private static final AtomicInteger sNextId = new AtomicInteger(1);
    private static final SparseArray<CameraSession> sSessions = new SparseArray<>();

    static class CameraSession {
        CameraManager manager;
        int resolution;
        volatile boolean disposed;
        volatile boolean switching;
        volatile int generation;
        final AtomicInteger pendingOpens = new AtomicInteger();
        CountDownLatch deviceClosed;
        volatile String cameraId;
        volatile CameraDevice device;
        volatile CameraCaptureSession captureSession;
        volatile CaptureRequest.Builder previewRequestBuilder;
        ImageReader imageReader;
        volatile ImageReader photoReader;
        MediaRecorder mediaRecorder;
        HandlerThread backgroundThread;
        Handler backgroundHandler;
        volatile TextureView textureView;
        volatile Surface previewSurface;
        volatile Size previewSize;
        boolean enableAudio;
        int flashMode = FLASH_OFF;
        String videoPath;
        volatile boolean isRecording;
        volatile float maxZoom = 1.0f;
        final Semaphore openLock = new Semaphore(1);
    }

    // ── Public API ───────────────────────────────────────────────────────

    /**
     * List available cameras.
     *
     * @return Array of "id|facing|orientation" strings.
     */
    public static String[] availableCameras(Activity activity) {
        try {
            CameraManager manager = (CameraManager) activity.getSystemService(Context.CAMERA_SERVICE);
            String[] ids = manager.getCameraIdList();
            String[] result = new String[ids.length];

            for (int i = 0; i < ids.length; i++) {
                CameraCharacteristics chars = manager.getCameraCharacteristics(ids[i]);
                Integer facing = chars.get(CameraCharacteristics.LENS_FACING);
                Integer orientation = chars.get(CameraCharacteristics.SENSOR_ORIENTATION);
                int facingInt = (facing != null) ? facing : -1;
                int orientInt = (orientation != null) ? orientation : 0;
                result[i] = ids[i] + "|" + facingInt + "|" + orientInt;
            }

            return result;
        } catch (CameraAccessException e) {
            android.util.Log.e(TAG, "Failed to list cameras", e);
            return new String[0];
        }
    }

    /**
     * Create a camera session.
     *
     * @return Session handle ID, or -1 on failure.
     */
    public static int createCamera(Activity activity, String cameraId, int resolution, boolean enableAudio) {
        CameraSession session = new CameraSession();
        try {
            session.enableAudio = enableAudio;
            session.resolution = resolution;
            session.manager = (CameraManager) activity.getApplicationContext().getSystemService(Context.CAMERA_SERVICE);

            // Start background thread
            session.backgroundThread = new HandlerThread("CameraBackground");
            session.backgroundThread.start();
            session.backgroundHandler = new Handler(session.backgroundThread.getLooper());

            openDevice(session, cameraId);

            int id = sNextId.getAndIncrement();
            synchronized (sSessions) {
                sSessions.put(id, session);
            }
            return id;

        } catch (Exception e) {
            if (e instanceof InterruptedException) Thread.currentThread().interrupt();
            session.disposed = true;
            closeDevice(session);
            quitWhenClosed(session);
            android.util.Log.e(TAG, "createCamera failed", e);
            return -1;
        }
    }

    /**
     * Start camera preview by adding a TextureView to the activity.
     */
    public static void startPreview(Activity activity, int handleId) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        if (session == null || session.device == null) return;

        final CameraSession s = session;
        CountDownLatch latch = new CountDownLatch(1);

        activity.runOnUiThread(() -> {
            try {
                // Create TextureView for preview
                TextureView tv = new TextureView(activity);
                FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.MATCH_PARENT);
                activity.addContentView(tv, params);
                s.textureView = tv;

                tv.setSurfaceTextureListener(new TextureView.SurfaceTextureListener() {
                    @Override
                    public void onSurfaceTextureAvailable(SurfaceTexture surface, int width, int height) {
                        attachPreviewSurface(s, surface);
                        latch.countDown();
                    }
                    @Override
                    public void onSurfaceTextureSizeChanged(SurfaceTexture surface, int w, int h) {}
                    @Override
                    public boolean onSurfaceTextureDestroyed(SurfaceTexture surface) {
                        releasePreviewSurface(s);
                        return true;
                    }
                    @Override
                    public void onSurfaceTextureUpdated(SurfaceTexture surface) {}
                });

                // If texture is already available
                if (tv.isAvailable()) {
                    SurfaceTexture st = tv.getSurfaceTexture();
                    attachPreviewSurface(s, st);
                    latch.countDown();
                }
            } catch (Exception e) {
                android.util.Log.e(TAG, "startPreview failed", e);
                latch.countDown();
            }
        });

        try {
            latch.await(5, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    /**
     * Stop the camera preview.
     */
    public static void stopPreview(int handleId) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        if (session == null) return;

        releasePreviewSurface(session);
        TextureView view = session.textureView;
        session.textureView = null;
        if (view != null) new Handler(Looper.getMainLooper()).post(() -> {
            view.setSurfaceTextureListener(null);
            ViewGroup parent = (ViewGroup) view.getParent();
            if (parent != null) parent.removeView(view);
        });
    }

    /**
     * Take a still photo.
     *
     * @return "path|width|height" or null on failure.
     */
    public static String takePicture(int handleId) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        if (session == null || session.device == null) return null;
        if (!session.openLock.tryAcquire()) return null;

        try {
            if (session.disposed || session.device == null) return null;
            CountDownLatch latch = new CountDownLatch(1);
            AtomicReference<String> resultRef = new AtomicReference<>(null);

            session.photoReader.setOnImageAvailableListener(reader -> {
                try (Image image = reader.acquireLatestImage()) {
                    if (image != null) {
                        ByteBuffer buffer = image.getPlanes()[0].getBuffer();
                        byte[] bytes = new byte[buffer.remaining()];
                        buffer.get(bytes);

                        String fileName = "photo_" + UUID.randomUUID().toString() + ".jpg";
                        File file = new File(Environment.getExternalStoragePublicDirectory(
                                Environment.DIRECTORY_PICTURES), fileName);
                        // Fallback to cache dir
                        if (!file.getParentFile().exists()) {
                            file = new File(System.getProperty("java.io.tmpdir"), fileName);
                        }

                        try (FileOutputStream fos = new FileOutputStream(file)) {
                            fos.write(bytes);
                        }

                        resultRef.set(file.getAbsolutePath() + "|" +
                                image.getWidth() + "|" + image.getHeight());
                    }
                } catch (IOException e) {
                    android.util.Log.e(TAG, "Failed to save photo", e);
                } finally {
                    latch.countDown();
                }
            }, session.backgroundHandler);

            CaptureRequest.Builder captureBuilder = session.device.createCaptureRequest(
                    CameraDevice.TEMPLATE_STILL_CAPTURE);
            captureBuilder.addTarget(session.photoReader.getSurface());
            captureBuilder.set(CaptureRequest.CONTROL_AF_MODE,
                    CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE);
            applyFlashMode(captureBuilder, session.flashMode);

            session.captureSession.capture(captureBuilder.build(),
                    new CameraCaptureSession.CaptureCallback() {}, session.backgroundHandler);

            latch.await(10, TimeUnit.SECONDS);
            return resultRef.get();

        } catch (Exception e) {
            android.util.Log.e(TAG, "takePicture failed", e);
            return null;
        } finally {
            session.openLock.release();
        }
    }

    /**
     * Start video recording.
     */
    public static void startVideoRecording(int handleId) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        if (session == null || session.device == null || session.isRecording) return;
        if (!session.openLock.tryAcquire()) throw new IllegalStateException("Camera operation in progress");

        try {
            if (session.disposed || session.device == null) throw new IllegalStateException("Camera was disposed");
            String fileName = "video_" + UUID.randomUUID().toString() + ".mp4";
            File file = new File(System.getProperty("java.io.tmpdir"), fileName);
            session.videoPath = file.getAbsolutePath();

            session.mediaRecorder = new MediaRecorder();
            if (session.enableAudio) {
                session.mediaRecorder.setAudioSource(MediaRecorder.AudioSource.MIC);
            }
            session.mediaRecorder.setVideoSource(MediaRecorder.VideoSource.SURFACE);
            session.mediaRecorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4);
            session.mediaRecorder.setVideoEncoder(MediaRecorder.VideoEncoder.H264);
            if (session.enableAudio) {
                session.mediaRecorder.setAudioEncoder(MediaRecorder.AudioEncoder.AAC);
            }
            session.mediaRecorder.setVideoSize(session.previewSize.getWidth(), session.previewSize.getHeight());
            session.mediaRecorder.setVideoFrameRate(30);
            session.mediaRecorder.setOutputFile(session.videoPath);
            session.mediaRecorder.prepare();

            // Close existing session
            if (session.captureSession != null) {
                session.captureSession.close();
                session.captureSession = null;
            }

            // Create new session with recorder surface
            List<Surface> surfaces = new ArrayList<>();
            if (session.previewSurface != null) surfaces.add(session.previewSurface);
            Surface recorderSurface = session.mediaRecorder.getSurface();
            surfaces.add(recorderSurface);

            CaptureRequest.Builder builder = session.device.createCaptureRequest(CameraDevice.TEMPLATE_RECORD);
            if (session.previewSurface != null) builder.addTarget(session.previewSurface);
            builder.addTarget(recorderSurface);
            builder.set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO);

            CountDownLatch latch = new CountDownLatch(1);
            AtomicBoolean accepted = new AtomicBoolean(true);
            AtomicReference<String> failure = new AtomicReference<>();
            final int generation = session.generation;

            session.device.createCaptureSession(surfaces, new CameraCaptureSession.StateCallback() {
                @Override
                public void onConfigured(CameraCaptureSession s) {
                    synchronized (session) {
                        try {
                            if (!accepted.get() || session.disposed || generation != session.generation) {
                                s.close();
                                failure.set("Recording configuration was superseded");
                                return;
                            }
                            session.captureSession = s;
                            s.setRepeatingRequest(builder.build(), null, session.backgroundHandler);
                            session.mediaRecorder.start();
                            session.isRecording = true;
                        } catch (Exception e) {
                            failure.set(e.toString());
                        } finally { latch.countDown(); }
                    }
                }

                @Override
                public void onConfigureFailed(CameraCaptureSession s) {
                    s.close();
                    failure.set("Recording session configure failed");
                    android.util.Log.e(TAG, "Recording session configure failed");
                    latch.countDown();
                }
            }, session.backgroundHandler);

            try {
                if (!latch.await(5, TimeUnit.SECONDS)) throw new IllegalStateException("Recording configuration timed out");
                if (failure.get() != null) throw new IllegalStateException(failure.get());
            } finally {
                synchronized (session) { accepted.set(false); }
            }

        } catch (Exception e) {
            if (e instanceof InterruptedException) Thread.currentThread().interrupt();
            synchronized (session) {
                session.generation++;
                if (session.captureSession != null) session.captureSession.close();
                session.captureSession = null;
                if (session.mediaRecorder != null) { session.mediaRecorder.release(); session.mediaRecorder = null; }
                session.isRecording = false;
            }
            createCaptureSession(session);
            throw new IllegalStateException("startVideoRecording failed", e);
        } finally {
            session.openLock.release();
        }
    }

    /**
     * Stop video recording.
     *
     * @return The video file path, or null on failure.
     */
    public static String stopVideoRecording(int handleId) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        if (session == null || !session.isRecording) return null;
        if (!session.openLock.tryAcquire()) return null;

        try {
            session.isRecording = false;
            session.mediaRecorder.stop();
            session.mediaRecorder.release();
            session.mediaRecorder = null;

            // Re-create preview session
            if (session.previewSurface != null) {
                createCaptureSession(session);
            }

            return session.videoPath;
        } catch (Exception e) {
            android.util.Log.e(TAG, "stopVideoRecording failed", e);
            return null;
        } finally {
            if (session.mediaRecorder != null) { session.mediaRecorder.release(); session.mediaRecorder = null; }
            session.openLock.release();
        }
    }

    public static void setFlashMode(int handleId, int mode) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        if (session == null) return;
        session.flashMode = mode;

        // Apply to active preview if running
        if (session.captureSession != null && session.previewRequestBuilder != null) {
            try {
                applyFlashMode(session.previewRequestBuilder, mode);
                session.captureSession.setRepeatingRequest(
                        session.previewRequestBuilder.build(), null, session.backgroundHandler);
            } catch (Exception e) {
                android.util.Log.e(TAG, "setFlashMode failed", e);
            }
        }
    }

    public static void setFocusMode(int handleId, int mode) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        if (session == null || session.previewRequestBuilder == null) return;

        try {
            int afMode = (mode == 0) ?
                    CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE :
                    CaptureRequest.CONTROL_AF_MODE_AUTO;
            session.previewRequestBuilder.set(CaptureRequest.CONTROL_AF_MODE, afMode);
            if (session.captureSession != null) {
                session.captureSession.setRepeatingRequest(
                        session.previewRequestBuilder.build(), null, session.backgroundHandler);
            }
        } catch (Exception e) {
            android.util.Log.e(TAG, "setFocusMode failed", e);
        }
    }

    public static void setExposureMode(int handleId, int mode) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        if (session == null || session.previewRequestBuilder == null) return;

        try {
            int aeMode = (mode == 0) ?
                    CaptureRequest.CONTROL_AE_MODE_ON :
                    CaptureRequest.CONTROL_AE_MODE_OFF;
            session.previewRequestBuilder.set(CaptureRequest.CONTROL_AE_MODE, aeMode);
            if (session.captureSession != null) {
                session.captureSession.setRepeatingRequest(
                        session.previewRequestBuilder.build(), null, session.backgroundHandler);
            }
        } catch (Exception e) {
            android.util.Log.e(TAG, "setExposureMode failed", e);
        }
    }

    public static float getMinZoom(int handleId) {
        return 1.0f;
    }

    public static float getMaxZoom(int handleId) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        return (session != null) ? session.maxZoom : 1.0f;
    }

    public static void setZoom(int handleId, float zoom) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        if (session == null || session.previewRequestBuilder == null) return;

        try {
            float clampedZoom = Math.max(1.0f, Math.min(zoom, session.maxZoom));
            // Digital zoom via SCALER_CROP_REGION
            // (simplified — a real implementation would compute the crop rect from sensor active area)
            session.previewRequestBuilder.set(CaptureRequest.CONTROL_ZOOM_RATIO, clampedZoom);
            if (session.captureSession != null) {
                session.captureSession.setRepeatingRequest(
                        session.previewRequestBuilder.build(), null, session.backgroundHandler);
            }
        } catch (Exception e) {
            android.util.Log.e(TAG, "setZoom failed", e);
        }
    }

    public static String setCamera(int handleId, String newCameraId) {
        CameraSession session;
        synchronized (sSessions) { session = sSessions.get(handleId); }
        if (session == null) return "Camera handle is not active";
        if (Looper.myLooper() == Looper.getMainLooper() ||
                Looper.myLooper() == session.backgroundHandler.getLooper()) {
            return "set_camera must run off the UI and camera callback threads";
        }
        if (!session.openLock.tryAcquire()) return "Camera operation is already in progress";
        String oldId = session.cameraId;
        boolean closed = false;
        try {
            if (session.disposed) return "Camera was disposed";
            if (session.isRecording) return "Stop video recording before switching cameras";
            if (newCameraId == null || newCameraId.isEmpty()) return "Camera ID is empty";
            if (newCameraId.equals(oldId) && session.device != null) return null;
            // Validate the new camera before disturbing the active one.
            session.manager.getCameraCharacteristics(newCameraId);
            session.switching = true;
            closed = true;
            closeAndWait(session);
            openDevice(session, newCameraId);
            restartPreview(session);
            return null;
        } catch (Exception error) {
            if (error instanceof InterruptedException) Thread.currentThread().interrupt();
            if (closed) {
                closeDevice(session);
                if (!Thread.currentThread().isInterrupted()) {
                    try {
                        if (session.deviceClosed != null && !session.deviceClosed.await(5, TimeUnit.SECONDS)) {
                            throw new IllegalStateException("Previous camera did not close");
                        }
                        openDevice(session, oldId);
                        restartPreview(session);
                    } catch (Exception restore) {
                        closeDevice(session);
                        return "Camera switch failed: " + error + "; restoring previous camera failed: " + restore;
                    }
                }
            }
            return "Camera switch failed: " + error;
        } finally {
            session.switching = false;
            session.openLock.release();
        }
    }

    public static void dispose(int handleId) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
            sSessions.remove(handleId);
        }
        if (session == null) return;

        session.openLock.acquireUninterruptibly();
        try {
            session.disposed = true;
            if (session.isRecording && session.mediaRecorder != null) {
                try { session.mediaRecorder.stop(); }
                catch (RuntimeException error) { android.util.Log.w(TAG, "Recorder stop failed", error); }
            }
            if (session.mediaRecorder != null) { session.mediaRecorder.release(); session.mediaRecorder = null; }
        } finally {
            closeDevice(session);
            releasePreviewSurface(session);
            TextureView view = session.textureView;
            session.textureView = null;
            if (view != null) new Handler(Looper.getMainLooper()).post(() -> {
                view.setSurfaceTextureListener(null);
                ViewGroup parent = (ViewGroup) view.getParent();
                if (parent != null) parent.removeView(view);
            });
            quitWhenClosed(session);
            session.openLock.release();
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    private static void attachPreviewSurface(CameraSession session, SurfaceTexture texture) {
        synchronized (session) {
            if (session.disposed || texture == null) return;
            if (session.previewSurface != null) session.previewSurface.release();
            texture.setDefaultBufferSize(session.previewSize.getWidth(), session.previewSize.getHeight());
            session.previewSurface = new Surface(texture);
            createCaptureSession(session);
        }
    }

    private static void releasePreviewSurface(CameraSession session) {
        synchronized (session) {
            session.generation++;
            if (session.captureSession != null) session.captureSession.close();
            session.captureSession = null;
            session.previewRequestBuilder = null;
            if (session.previewSurface != null) session.previewSurface.release();
            session.previewSurface = null;
        }
    }

    private static void openDevice(CameraSession session, String cameraId) throws Exception {
        CameraCharacteristics chars = session.manager.getCameraCharacteristics(cameraId);
        StreamConfigurationMap map = chars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP);
        if (map == null) throw new IllegalStateException("Camera has no stream configurations");
        Size preview = chooseSize(map.getOutputSizes(SurfaceTexture.class), session.resolution);
        Size photo = chooseSize(map.getOutputSizes(ImageFormat.JPEG), session.resolution);
        Float maxZoom = chars.get(CameraCharacteristics.SCALER_AVAILABLE_MAX_DIGITAL_ZOOM);
        session.previewSize = preview;
        session.maxZoom = maxZoom == null ? 1.0f : maxZoom;
        session.photoReader = ImageReader.newInstance(photo.getWidth(), photo.getHeight(), ImageFormat.JPEG, 2);
        CountDownLatch opened = new CountDownLatch(1);
        AtomicReference<String> failure = new AtomicReference<>();
        final int generation = session.generation;
        CountDownLatch deviceClosed = new CountDownLatch(1);
        session.deviceClosed = deviceClosed;
        AtomicBoolean pending = new AtomicBoolean(true);
        session.pendingOpens.incrementAndGet();
        try {
        session.manager.openCamera(cameraId, new CameraDevice.StateCallback() {
            @Override public void onOpened(CameraDevice device) {
                synchronized (session) {
                    if (session.disposed || session.generation != generation) device.close();
                    else session.device = device;
                }
                finished();
                opened.countDown();
            }
            @Override public void onClosed(CameraDevice device) { deviceClosed.countDown(); }
            @Override public void onDisconnected(CameraDevice device) { failed(device, "Camera disconnected"); }
            @Override public void onError(CameraDevice device, int error) { failed(device, "Camera error " + error); }
            private void failed(CameraDevice device, String error) {
                synchronized (session) {
                    device.close();
                    if (session.device == device) session.device = null;
                    failure.set(error);
                }
                finished();
                opened.countDown();
            }
            private void finished() {
                if (pending.compareAndSet(true, false)) session.pendingOpens.decrementAndGet();
                quitWhenClosed(session);
            }
        }, session.backgroundHandler);
        } catch (Exception error) {
            if (pending.compareAndSet(true, false)) session.pendingOpens.decrementAndGet();
            deviceClosed.countDown();
            throw error;
        }
        if (!opened.await(5, TimeUnit.SECONDS)) throw new IllegalStateException("Opening camera timed out");
        synchronized (session) {
            if (session.device == null || failure.get() != null) throw new IllegalStateException(failure.get());
            session.cameraId = cameraId;
        }
    }

    private static void quitWhenClosed(CameraSession session) {
        // Keep the callback looper alive until late opens can close their device.
        if (session.disposed && session.pendingOpens.get() == 0 && session.backgroundThread != null) {
            session.backgroundThread.quitSafely();
        }
    }

    private static void closeAndWait(CameraSession session) throws InterruptedException {
        CountDownLatch closed = session.deviceClosed;
        closeDevice(session);
        if (closed != null && !closed.await(5, TimeUnit.SECONDS)) {
            throw new IllegalStateException("Camera close timed out");
        }
    }

    private static void closeDevice(CameraSession session) {
        synchronized (session) {
            session.generation++;
            if (session.captureSession != null) session.captureSession.close();
            session.captureSession = null;
            session.previewRequestBuilder = null;
            if (session.device != null) session.device.close();
            session.device = null;
            if (session.photoReader != null) session.photoReader.close();
            session.photoReader = null;
        }
    }

    private static void restartPreview(CameraSession session) throws Exception {
        if (session.previewSurface == null) return;
        TextureView view = session.textureView;
        SurfaceTexture texture = view == null ? null : view.getSurfaceTexture();
        if (texture == null) throw new IllegalStateException("Preview surface was destroyed");
        texture.setDefaultBufferSize(session.previewSize.getWidth(), session.previewSize.getHeight());
        CountDownLatch configured = new CountDownLatch(1);
        AtomicReference<String> error = new AtomicReference<>();
        createCaptureSession(session, configured, error);
        if (!configured.await(5, TimeUnit.SECONDS)) throw new IllegalStateException("Preview configuration timed out");
        if (error.get() != null) throw new IllegalStateException(error.get());
    }

    private static void createCaptureSession(CameraSession session) {
        if (session.switching || session.disposed) return;
        createCaptureSession(session, new CountDownLatch(0), new AtomicReference<>());
    }

    private static void createCaptureSession(CameraSession session, CountDownLatch configured,
                                              AtomicReference<String> error) {
        final int generation = session.generation;
        try {
            if (session.device == null || session.previewSurface == null || session.photoReader == null) {
                throw new IllegalStateException("Camera preview is unavailable");
            }
            List<Surface> surfaces = new ArrayList<>();
            surfaces.add(session.previewSurface);
            surfaces.add(session.photoReader.getSurface());

            CaptureRequest.Builder builder = session.device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW);
            builder.addTarget(session.previewSurface);
            builder.set(CaptureRequest.CONTROL_AF_MODE,
                    CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE);
            applyFlashMode(builder, session.flashMode);

            session.device.createCaptureSession(surfaces, new CameraCaptureSession.StateCallback() {
                @Override
                public void onConfigured(CameraCaptureSession s) {
                    synchronized (session) {
                        try {
                            if (session.disposed || generation != session.generation) {
                                s.close();
                                error.set("Preview configuration was superseded");
                                return;
                            }
                            s.setRepeatingRequest(builder.build(), null, session.backgroundHandler);
                            session.captureSession = s;
                            session.previewRequestBuilder = builder;
                        } catch (Exception e) {
                            s.close();
                            error.set(e.toString());
                            android.util.Log.e(TAG, "Failed to start preview", e);
                        } finally { configured.countDown(); }
                    }
                }

                @Override
                public void onConfigureFailed(CameraCaptureSession s) {
                    s.close();
                    error.set("Capture session configure failed");
                    configured.countDown();
                    android.util.Log.e(TAG, "Capture session configure failed");
                }
            }, session.backgroundHandler);

        } catch (Exception e) {
            error.set(e.toString());
            configured.countDown();
            android.util.Log.e(TAG, "createCaptureSession failed", e);
        }
    }

    private static Size chooseSize(Size[] sizes, int resolution) {
        if (sizes == null || sizes.length == 0) throw new IllegalArgumentException("No supported stream size");
        if (resolution == RES_MAX) {
            Size best = sizes[0];
            for (Size size : sizes) {
                if ((long) size.getWidth() * size.getHeight() > (long) best.getWidth() * best.getHeight()) best = size;
            }
            return best;
        }
        int targetWidth, targetHeight;
        switch (resolution) {
            case RES_LOW:       targetWidth = 320;  targetHeight = 240;  break;
            case RES_MEDIUM:    targetWidth = 640;  targetHeight = 480;  break;
            case RES_HIGH:      targetWidth = 1280; targetHeight = 720;  break;
            case RES_VERY_HIGH: targetWidth = 1920; targetHeight = 1080; break;
            case RES_ULTRA_HIGH:targetWidth = 3840; targetHeight = 2160; break;
            case RES_MAX:       targetWidth = Integer.MAX_VALUE; targetHeight = Integer.MAX_VALUE; break;
            default:            targetWidth = 1280; targetHeight = 720;  break;
        }

        Size best = sizes[0];
        int bestDiff = Integer.MAX_VALUE;

        for (Size size : sizes) {
            int diff = Math.abs(size.getWidth() - targetWidth) + Math.abs(size.getHeight() - targetHeight);
            if (diff < bestDiff) {
                bestDiff = diff;
                best = size;
            }
        }

        return best;
    }

    private static void applyFlashMode(CaptureRequest.Builder builder, int flashMode) {
        switch (flashMode) {
            case FLASH_OFF:
                builder.set(CaptureRequest.FLASH_MODE, CaptureRequest.FLASH_MODE_OFF);
                builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_ON);
                break;
            case FLASH_AUTO:
                builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_ON_AUTO_FLASH);
                break;
            case FLASH_ALWAYS:
                builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_ON_ALWAYS_FLASH);
                break;
            case FLASH_TORCH:
                builder.set(CaptureRequest.FLASH_MODE, CaptureRequest.FLASH_MODE_TORCH);
                builder.set(CaptureRequest.CONTROL_AE_MODE, CaptureRequest.CONTROL_AE_MODE_ON);
                break;
        }
    }

    /**
     * Create a TextureView for camera preview.
     *
     * <p>The TextureView is wired to the CameraSession. When the surface
     * becomes available, a capture session is created automatically.
     * The view is NOT added to the hierarchy — the caller (GpuiPlatformView) handles that.</p>
     *
     * @param activity  The current Activity.
     * @param handleId  Camera session handle ID.
     * @return A configured TextureView, or an empty FrameLayout if the session is not found.
     */
    public static android.view.View createPreviewSurface(Activity activity, int handleId) {
        CameraSession session;
        synchronized (sSessions) {
            session = sSessions.get(handleId);
        }
        if (session == null || session.device == null) {
            android.util.Log.w(TAG, "createPreviewSurface: session " + handleId + " not found");
            return new android.widget.FrameLayout(activity);
        }

        final CameraSession s = session;
        android.view.TextureView tv = new android.view.TextureView(activity);
        s.textureView = tv;

        tv.setSurfaceTextureListener(new android.view.TextureView.SurfaceTextureListener() {
            @Override
            public void onSurfaceTextureAvailable(android.graphics.SurfaceTexture surface, int width, int height) {
                attachPreviewSurface(s, surface);
            }

            @Override
            public void onSurfaceTextureSizeChanged(android.graphics.SurfaceTexture surface, int w, int h) {}

            @Override
            public boolean onSurfaceTextureDestroyed(android.graphics.SurfaceTexture surface) {
                releasePreviewSurface(s);
                return true;
            }

            @Override
            public void onSurfaceTextureUpdated(android.graphics.SurfaceTexture surface) {}
        });

        // If texture is already available
        if (tv.isAvailable()) {
            android.graphics.SurfaceTexture st = tv.getSurfaceTexture();
            attachPreviewSurface(s, st);
        }

        // Start the session running
        // (capture session creation is async via the SurfaceTextureListener)

        return tv;
    }

    private GpuiCamera() {}
}
