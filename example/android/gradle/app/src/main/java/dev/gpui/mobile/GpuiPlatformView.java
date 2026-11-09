package dev.gpui.mobile;

import android.app.Activity;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.view.View;
import android.view.ViewGroup;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.webkit.WebChromeClient;
import android.widget.FrameLayout;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.Callable;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.TimeUnit;
import com.google.android.gms.maps.MapView;
import com.google.android.gms.maps.GoogleMap;
import com.google.android.gms.maps.CameraUpdateFactory;
import com.google.android.gms.maps.model.LatLng;
import com.google.android.gms.maps.model.Marker;
import com.google.android.gms.maps.model.MarkerOptions;
import com.google.android.gms.common.GoogleApiAvailability;

/**
 * Helper class for managing native platform views embedded in the GPUI render tree.
 *
 * Platform views are native Android Views positioned absolutely over the
 * NativeActivity's content area. GPUI controls their position and visibility
 * via JNI calls from Rust.
 *
 * Supports view types:
 * - "container": Generic empty FrameLayout (for custom content)
 * - Additional types can be registered via registerViewType()
 */
public class GpuiPlatformView {
    private static final String TAG = "GpuiPlatformView";
    private static final Handler mainHandler = new Handler(Looper.getMainLooper());

    /** Map of view ID -> native View */
    private static final Map<Long, View> views = new HashMap<>();

    /** Map of view ID -> FrameLayout container */
    private static final Map<Long, FrameLayout> containers = new HashMap<>();

    /** The root FrameLayout that hosts all platform views */
    private static FrameLayout rootContainer;
    private static final Map<Long, Boolean> requestedVisibility = new HashMap<>();
    private static boolean paused;

    // A timeout must distinguish queued work (no effects) from native work already running.
    // Do not interrupt Android UI operations. Retire their instance when late work returns.
    static final class MainCall<T> implements Runnable {
        private enum State { QUEUED, RUNNING, COMPLETE, CANCELLED, ABANDONED }
        private State state = State.QUEUED;
        private final Callable<T> action;
        private final Runnable lateCleanup;
        private final CountDownLatch done = new CountDownLatch(1);
        private T result;
        private Throwable error;

        MainCall(Callable<T> action, Runnable lateCleanup) {
            this.action = action;
            this.lateCleanup = lateCleanup;
        }

        public void run() {
            synchronized (this) {
                if (state != State.QUEUED) return;
                state = State.RUNNING;
            }
            T value = null;
            Throwable failure = null;
            try { value = action.call(); } catch (Throwable e) { failure = e; }
            boolean abandoned;
            synchronized (this) {
                abandoned = state == State.ABANDONED;
                if (!abandoned) {
                    result = value;
                    error = failure;
                    state = State.COMPLETE;
                }
            }
            try { if (abandoned) lateCleanup.run(); }
            catch (RuntimeException e) { Log.e(TAG, "Late native cleanup failed", e); }
            finally { done.countDown(); }
        }

        T await(long milliseconds) throws Exception {
            try { done.await(milliseconds, TimeUnit.MILLISECONDS); }
            catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                synchronized (this) {
                    if (state == State.QUEUED) state = State.CANCELLED;
                    else if (state == State.RUNNING) state = State.ABANDONED;
                    else if (state == State.COMPLETE) return completed();
                }
                throw e;
            }
            synchronized (this) {
                if (state == State.COMPLETE) return completed();
                boolean queued = state == State.QUEUED;
                state = queued ? State.CANCELLED : State.ABANDONED;
                throw new TimeoutException(queued
                    ? "Android UI dispatch timed out before execution; operation cancelled"
                    : "Android UI operation timed out while running; late effects are possible; instance will be disposed");
            }
        }

        private T completed() throws ExecutionException {
            if (error != null) throw new ExecutionException(error);
            return result;
        }
    }

    private static <T> T onMain(Callable<T> action, Runnable lateCleanup) throws Exception {
        if (Looper.myLooper() == Looper.getMainLooper()) return action.call();
        MainCall<T> task = new MainCall<>(action, lateCleanup);
        if (!mainHandler.post(task)) throw new IllegalStateException("Android UI thread is unavailable");
        try { return task.await(1000); }
        finally { mainHandler.removeCallbacks(task); }
    }

    private static String failure(Exception error) {
        Throwable cause = error;
        while (cause.getCause() != null) cause = cause.getCause();
        return cause.getClass().getSimpleName() + ": " + cause.getMessage();
    }

    /**
     * Ensure the root container exists in the activity's view hierarchy.
     * Platform views are added as children of this container.
     */
    private static void ensureRootContainer(Activity activity) {
        if (rootContainer != null) {
            return;
        }

        {
            if (rootContainer != null) return;

            rootContainer = new FrameLayout(activity);
            rootContainer.setLayoutParams(new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            ));

            // Add to the activity's content view on top of the NativeActivity surface.
            // NativeActivity uses an internal SurfaceView for native rendering.
            // addContentView adds to the end of the window's DecorView, which
            // renders ON TOP of the NativeActivity's SurfaceView.
            activity.addContentView(rootContainer, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            ));

            // Log the view hierarchy for debugging
            try {
                View decorView = activity.getWindow().getDecorView();
                logViewHierarchy(decorView, 0);
            } catch (Exception e) {
                Log.w(TAG, "Could not log view hierarchy: " + e.getMessage());
            }

            Log.i(TAG, "Root container created and added to activity");
            activity.getApplication().registerActivityLifecycleCallbacks(new android.app.Application.ActivityLifecycleCallbacks() {
                public void onActivityCreated(Activity a, android.os.Bundle state) {}
                public void onActivityStarted(Activity a) {}
                public void onActivityResumed(Activity a) { if (a == activity) resumeAll(); }
                public void onActivityPaused(Activity a) { if (a == activity) pauseAll(); }
                public void onActivityStopped(Activity a) {}
                public void onActivitySaveInstanceState(Activity a, android.os.Bundle state) {}
                public void onActivityDestroyed(Activity a) {
                    if (a != activity) return;
                    disposeAll();
                    activity.getApplication().unregisterActivityLifecycleCallbacks(this);
                }
            });
        }
    }

    /**
     * Create a new platform view and add it to the view hierarchy.
     *
     * @param activity       The hosting Activity
     * @param viewType       The type of view to create (e.g., "container", "video_player", "webview")
     * @param viewId         Unique ID for this view instance
     * @param x              Left position in logical pixels
     * @param y              Top position in logical pixels
     * @param width          Width in logical pixels
     * @param height         Height in logical pixels
     * @param creationParams Pipe-delimited key=value pairs (e.g., "player_id=1|url=https://...")
     * @return true if the view was created successfully
     */
    public static boolean createView(
            Activity activity,
            String viewType,
            long viewId,
            float x, float y,
            float width, float height,
            String creationParams) {

        return createViewChecked(activity, viewType, viewId, x, y, width, height, creationParams) == null;
    }

    public static String createViewChecked(Activity activity, String viewType, long viewId,
            float x, float y, float width, float height, String creationParams) {
        final View[] created = new View[1];
        try {
            return onMain(() -> {
                if (activity == null || activity.isFinishing() || activity.isDestroyed())
                    throw new IllegalStateException("No active Activity for platform view");
                if (views.containsKey(viewId)) throw new IllegalArgumentException("Duplicate view ID");
                Map<String, String> params = parseCreationParams(creationParams);
                ensureRootContainer(activity);
                if (rootContainer.getContext() != activity)
                    throw new IllegalStateException("Platform views belong to a different Activity");
                float density = activity.getResources().getDisplayMetrics().density;

                // Create a container FrameLayout for this platform view
                FrameLayout container = new FrameLayout(activity);
                FrameLayout.LayoutParams layoutParams = new FrameLayout.LayoutParams(
                    (int)(width * density),
                    (int)(height * density)
                );
                layoutParams.leftMargin = (int)(x * density);
                layoutParams.topMargin = (int)(y * density);
                container.setLayoutParams(layoutParams);

                // Create the actual view based on type
                View view = createViewForType(activity, viewType, params);
                created[0] = view;
                try {
                    if (view == null) throw new IllegalStateException("Native view factory returned null");
                    container.addView(view, new FrameLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        ViewGroup.LayoutParams.MATCH_PARENT
                    ));
                    views.put(viewId, view);
                    containers.put(viewId, container);
                    requestedVisibility.put(viewId, true);
                    container.setVisibility(paused ? View.INVISIBLE : View.VISIBLE);
                    rootContainer.addView(container);
                    Log.i(TAG, "View created: id=" + viewId + " type=" + viewType);
                    return null;
                } catch (RuntimeException e) {
                    views.remove(viewId); containers.remove(viewId);
                    requestedVisibility.remove(viewId);
                    rootContainer.removeView(container); container.removeAllViews();
                    destroyNative(view);
                    throw e;
                }
            }, () -> {
                if (created[0] != null && views.get(viewId) == created[0]) disposeViewOnMain(viewId);
            });
        } catch (Exception e) { return failure(e); }
        catch (LinkageError e) { return "Native map SDK missing: add com.google.android.gms:play-services-maps:20.0.0 (" + e + ")"; }
    }

    /**
     * Parse a pipe-delimited creation params string into a map.
     * Format: "key1=value1|key2=value2"
     */
    private static Map<String, String> parseCreationParams(String params) {
        Map<String, String> map = new HashMap<>();
        if (params == null || params.isEmpty()) return map;
        for (String pair : params.split("\\|")) {
            int eq = pair.indexOf('=');
            if (eq > 0) {
                map.put(decode(pair.substring(0, eq)), decode(pair.substring(eq + 1)));
            }
        }
        return map;
    }

    private static String decode(String value) {
        return value.replace("%3D", "=").replace("%7C", "|").replace("%25", "%");
    }

    private static Activity checkedMapActivity(Activity activity, Map<String, String> params) {
        try {
            android.content.pm.ApplicationInfo info = activity.getPackageManager()
                .getApplicationInfo(activity.getPackageName(), android.content.pm.PackageManager.GET_META_DATA);
            String key = info.metaData == null ? null : info.metaData.getString("com.google.android.geo.API_KEY");
            if (key == null || key.trim().isEmpty() || key.contains("${") || key.equals("YOUR_API_KEY"))
                throw new IllegalStateException("Maps requires host manifest com.google.android.geo.API_KEY");
            int status = GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(activity);
            if (status != 0) throw new IllegalStateException("Google Play services unavailable: " + status);
            if (Boolean.parseBoolean(params.getOrDefault("my_location", "false"))
                    && activity.checkSelfPermission("android.permission.ACCESS_FINE_LOCATION") != 0
                    && activity.checkSelfPermission("android.permission.ACCESS_COARSE_LOCATION") != 0)
                throw new SecurityException("Map user location requires existing location permission");
            coordinate(params.get("latitude"), params.get("longitude"));
            zoomLevel(params.get("zoom"));
            nativeMapType(Integer.parseInt(params.getOrDefault("map_type", "0")));
            return activity;
        } catch (Exception e) { throw new IllegalStateException("Map configuration: " + failure(e), e); }
    }

    private static final class NativeMap extends MapView {
        GoogleMap map;
        String error;
        boolean disposed;
        boolean resumed;
        final Map<String, Marker> markers = new HashMap<>();

        NativeMap(Activity activity, Map<String, String> params) {
            super(checkedMapActivity(activity, params));
            try {
                boolean location = Boolean.parseBoolean(params.getOrDefault("my_location", "false"));
                onCreate(null);
                resumeMap();
                getMapAsync(ready -> {
                    if (disposed) return;
                    try {
                        ready.getUiSettings().setZoomGesturesEnabled(Boolean.parseBoolean(params.getOrDefault("zoom_gestures", "true")));
                        ready.getUiSettings().setScrollGesturesEnabled(Boolean.parseBoolean(params.getOrDefault("scroll_gestures", "true")));
                        ready.getUiSettings().setRotateGesturesEnabled(Boolean.parseBoolean(params.getOrDefault("rotate_gestures", "true")));
                        ready.setMapType(nativeMapType(Integer.parseInt(params.getOrDefault("map_type", "0"))));
                        ready.setMyLocationEnabled(location);
                        ready.moveCamera(CameraUpdateFactory.newLatLngZoom(
                            coordinate(params.get("latitude"), params.get("longitude")),
                            zoomLevel(params.get("zoom"))));
                        map = ready;
                    } catch (Exception e) { error = failure(e); }
                });
            } catch (Exception e) {
                disposed = true;
                pauseMap();
                onDestroy();
                throw new IllegalStateException("Map creation failed: " + failure(e), e);
            }
        }
        void resumeMap() { if (!resumed && !disposed) { onStart(); onResume(); resumed = true; } }
        void pauseMap() { if (resumed) { onPause(); onStop(); resumed = false; } }
        void destroyMap() {
            if (disposed) return;
            pauseMap(); disposed = true; markers.clear(); map = null; onDestroy();
        }
        GoogleMap ready() {
            if (disposed) throw new IllegalStateException("Map is disposed");
            if (error != null) throw new IllegalStateException(error);
            if (map == null) throw new IllegalStateException("Map is not ready; retry after native map initialization");
            return map;
        }
    }

    private static int nativeMapType(int kind) {
        switch (kind) {
            case 0: return GoogleMap.MAP_TYPE_NORMAL;
            case 1: return GoogleMap.MAP_TYPE_SATELLITE;
            case 2: return GoogleMap.MAP_TYPE_HYBRID;
            case 3: return GoogleMap.MAP_TYPE_TERRAIN;
            default: throw new IllegalArgumentException("Invalid map type");
        }
    }

    private static LatLng coordinate(String latitude, String longitude) {
        double lat = Double.parseDouble(latitude), lng = Double.parseDouble(longitude);
        if (!Double.isFinite(lat) || !Double.isFinite(lng) || lat < -90 || lat > 90 || lng < -180 || lng > 180)
            throw new IllegalArgumentException("Invalid map coordinate");
        return new LatLng(lat, lng);
    }
    private static float zoomLevel(String text) {
        float zoom = Float.parseFloat(text);
        if (!Float.isFinite(zoom) || zoom < 0 || zoom > 20) throw new IllegalArgumentException("Invalid map zoom");
        return zoom;
    }

    /** Null means native completion; any non-null value is an error returned to Rust. */
    public static String command(long id, String name, String payload) {
        String[] args = payload.split("\\|", -1);
        for (int i = 0; i < args.length; i++) args[i] = decode(args[i]);
        if (name.equals("webview.evaluate")) return evaluate(id, args[0]);
        try {
            return onMain(() -> {
                View view = views.get(id);
                if (view == null) throw new IllegalStateException("Platform view is disposed or missing");
                if (name.startsWith("webview.")) {
                    if (!(view instanceof WebView)) throw new IllegalArgumentException("View is not a WebView");
                    WebView web = (WebView) view;
                    switch (name) {
                        case "webview.back":
                            if (!web.canGoBack()) throw new IllegalStateException("WebView has no back history");
                            web.goBack(); break;
                        case "webview.reload": web.reload(); break;
                        case "webview.stop": web.stopLoading(); break;
                        default: throw new IllegalArgumentException("Unknown WebView command");
                    }
                } else {
                    if (!(view instanceof NativeMap)) throw new IllegalArgumentException("View is not a map");
                    NativeMap state = (NativeMap) view;
                    GoogleMap map = state.ready();
                    switch (name) {
                        case "map.center": map.moveCamera(CameraUpdateFactory.newLatLng(coordinate(args[0], args[1]))); break;
                        case "map.zoom": map.moveCamera(CameraUpdateFactory.zoomTo(zoomLevel(args[0]))); break;
                        case "map.type": map.setMapType(nativeMapType(Integer.parseInt(args[0]))); break;
                        case "map.add":
                            if (args[0].isEmpty()) throw new IllegalArgumentException("Marker ID must not be empty");
                            Marker marker = map.addMarker(new MarkerOptions().position(coordinate(args[1], args[2]))
                                .title(args[3]).snippet(args[4]));
                            if (marker == null) throw new IllegalStateException("Native marker creation failed");
                            Marker old = state.markers.put(args[0], marker);
                            if (old != null) old.remove();
                            break;
                        case "map.remove":
                            Marker removed = state.markers.remove(args[0]);
                            if (removed == null) throw new IllegalArgumentException("Unknown marker ID");
                            removed.remove(); break;
                        case "map.clear":
                            for (Marker item : state.markers.values()) item.remove();
                            state.markers.clear(); break;
                        default: throw new IllegalArgumentException("Unknown map command");
                    }
                }
                return null;
            }, () -> disposeViewOnMain(id));
        } catch (Exception e) { return failure(e); }
    }

    private static String evaluate(long id, String script) {
        if (Looper.myLooper() == Looper.getMainLooper())
            return "Synchronous JavaScript evaluation must run off the Android UI thread";
        java.util.concurrent.CompletableFuture<String> completion = new java.util.concurrent.CompletableFuture<>();
        try {
            onMain(() -> {
                View view = views.get(id);
                if (!(view instanceof WebView)) throw new IllegalStateException("No active WebView instance");
                WebView web = (WebView) view;
                if (!web.getSettings().getJavaScriptEnabled()) throw new IllegalStateException("JavaScript is disabled");
                // Android's callback has no exception argument; evaluate in a wrapper that reports thrown errors.
                String source = "(function(){try{(0,eval)(" + org.json.JSONObject.quote(script)
                    + ");return {ok:true};}catch(e){return {ok:false,error:String(e)};}})()";
                web.evaluateJavascript(source, result -> {
                    try {
                        if (views.get(id) != web) { completion.complete("WebView was disposed during evaluation"); return; }
                        org.json.JSONObject status = new org.json.JSONObject(result);
                        completion.complete(status.getBoolean("ok") ? null : status.getString("error"));
                    } catch (Exception e) { completion.complete("Invalid JavaScript completion: " + failure(e)); }
                });
                return null;
            }, () -> disposeViewOnMain(id));
            try { return completion.get(1, TimeUnit.SECONDS); }
            catch (TimeoutException e) {
                disposeView(id);
                return "JavaScript completion timed out; execution may have occurred; instance scheduled for disposal";
            } catch (InterruptedException e) {
                disposeView(id);
                Thread.currentThread().interrupt();
                return "JavaScript wait interrupted; execution may have occurred; instance scheduled for disposal";
            }
        } catch (Exception e) { return failure(e); }
    }

    /**
     * Create a View instance based on the view type string and creation params.
     */
    private static View createViewForType(Activity activity, String viewType, Map<String, String> params) {
        switch (viewType) {
            case "container":
                FrameLayout frame = new FrameLayout(activity);
                frame.setBackgroundColor(0x00000000);
                return frame;

            case "video_player":
                return createVideoPlayerView(activity, params);

            case "webview":
                return createWebViewView(activity, params);

            case "camera_preview":
                return createCameraPreviewView(activity, params);

            case "map":
                return new NativeMap(activity, params);

            default:
                Log.w(TAG, "Unknown view type: " + viewType + ", creating empty container");
                return new FrameLayout(activity);
        }
    }

    /**
     * Create a TextureView for video playback and wire it to the MediaPlayer.
     */
    private static View createVideoPlayerView(Activity activity, Map<String, String> params) {
        int playerId = 0;
        try {
            playerId = Integer.parseInt(params.getOrDefault("player_id", "0"));
        } catch (NumberFormatException e) {
            Log.w(TAG, "Invalid player_id in creation params");
        }

        return GpuiVideoPlayer.createVideoSurface(activity, playerId);
    }

    /**
     * Create a WebView with settings from creation params.
     */
    private static View createWebViewView(Activity activity, Map<String, String> params) {
        boolean jsEnabled = Boolean.parseBoolean(params.getOrDefault("javascript_enabled", "true"));
        boolean domStorage = Boolean.parseBoolean(params.getOrDefault("dom_storage_enabled", "true"));
        boolean zoom = Boolean.parseBoolean(params.getOrDefault("zoom_enabled", "true"));
        String url = params.getOrDefault("url", "");
        String html = params.getOrDefault("html", "");

        WebView wv = new WebView(activity);
        WebSettings settings = wv.getSettings();
        settings.setJavaScriptEnabled(jsEnabled);
        settings.setDomStorageEnabled(domStorage);
        settings.setBuiltInZoomControls(zoom);
        if (zoom) settings.setDisplayZoomControls(false);
        settings.setLoadWithOverviewMode(true);
        settings.setUseWideViewPort(true);
        if (params.containsKey("user_agent")) settings.setUserAgentString(params.get("user_agent"));

        wv.setWebViewClient(new WebViewClient());
        wv.setWebChromeClient(new WebChromeClient());

        if (!html.isEmpty()) {
            wv.loadDataWithBaseURL(null, html, "text/html", "UTF-8", null);
        } else if (!url.isEmpty()) {
            wv.loadUrl(url);
        }

        return wv;
    }

    /**
     * Create a TextureView for camera preview and wire it to the camera session.
     */
    private static View createCameraPreviewView(Activity activity, Map<String, String> params) {
        int sessionId = 0;
        try {
            sessionId = Integer.parseInt(params.getOrDefault("session_id", "0"));
        } catch (NumberFormatException e) {
            Log.w(TAG, "Invalid session_id in creation params");
        }

        return GpuiCamera.createPreviewSurface(activity, sessionId);
    }

    /**
     * Update a view's position and size.
     */
    public static void setBounds(long viewId, float x, float y, float width, float height) {
        mainHandler.post(() -> {
            FrameLayout container = containers.get(viewId);
            if (container == null) {
                Log.w(TAG, "setBounds: no container for id=" + viewId);
                return;
            }

            Activity activity = (Activity) container.getContext();
            float density = activity.getResources().getDisplayMetrics().density;

            FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) container.getLayoutParams();
            params.leftMargin = (int)(x * density);
            params.topMargin = (int)(y * density);
            params.width = (int)(width * density);
            params.height = (int)(height * density);
            container.setLayoutParams(params);
        });
    }

    /**
     * Show or hide a platform view.
     */
    public static void setVisible(long viewId, boolean visible) {
        mainHandler.post(() -> {
            FrameLayout container = containers.get(viewId);
            if (container == null) return;
            requestedVisibility.put(viewId, visible);
            container.setVisibility(paused ? View.INVISIBLE : visible ? View.VISIBLE : View.GONE);
        });
    }

    /**
     * Set the z-order of a platform view.
     * Uses View.setZ() (API 21+) for elevation-based ordering.
     */
    public static void setZIndex(long viewId, int zIndex) {
        mainHandler.post(() -> {
            FrameLayout container = containers.get(viewId);
            if (container == null) return;
            container.setZ(zIndex);
        });
    }

    /**
     * Remove and dispose a platform view.
     */
    public static void disposeView(long viewId) {
        // NativeActivity teardown may be waiting for the native caller. Never wait back on it.
        if (Looper.myLooper() == Looper.getMainLooper()) disposeViewOnMain(viewId);
        else if (!mainHandler.post(() -> disposeViewOnMain(viewId)))
            Log.e(TAG, "UI queue unavailable for platform view disposal: " + viewId);
    }

    private static void disposeViewOnMain(long viewId) {
            View view = views.remove(viewId);
            requestedVisibility.remove(viewId);
            FrameLayout container = containers.get(viewId);
            if (container != null) {
                if (rootContainer != null) {
                    rootContainer.removeView(container);
                }
                container.removeAllViews();
                containers.remove(viewId);
            }
            destroyNative(view);
            Log.i(TAG, "View disposed: id=" + viewId);
    }

    private static void destroyNative(View view) {
        try {
            if (view instanceof WebView) {
                WebView web = (WebView) view;
                try { web.stopLoading(); web.setWebChromeClient(null); web.setWebViewClient(null); }
                finally { web.destroy(); }
            } else if (view instanceof NativeMap) {
                ((NativeMap) view).destroyMap();
            }
        } catch (RuntimeException error) { Log.e(TAG, "Native view destruction failed", error); }
    }

    /**
     * Get the native View for a given platform view ID.
     * Useful for packages that need direct access to the Android View.
     */
    public static View getView(long viewId) {
        return views.get(viewId);
    }

    /**
     * Get the container FrameLayout for a given platform view ID.
     */
    public static FrameLayout getContainer(long viewId) {
        return containers.get(viewId);
    }

    /**
     * Pause all platform views. Called when the app goes to background.
     * Views are hidden to release rendering resources.
     */
    public static void pauseAll() {
        mainHandler.post(() -> {
            paused = true;
            for (View view : views.values()) {
                if (view instanceof WebView) ((WebView) view).onPause();
                if (view instanceof NativeMap) ((NativeMap) view).pauseMap();
            }
            for (FrameLayout container : containers.values()) {
                container.setVisibility(View.INVISIBLE);
            }
            Log.i(TAG, "All platform views paused");
        });
    }

    /**
     * Resume all platform views. Called when the app returns to foreground.
     * Views are made visible again.
     */
    public static void resumeAll() {
        mainHandler.post(() -> {
            paused = false;
            for (View view : views.values()) {
                if (view instanceof WebView) ((WebView) view).onResume();
                if (view instanceof NativeMap) ((NativeMap) view).resumeMap();
            }
            for (Map.Entry<Long, FrameLayout> entry : containers.entrySet()) {
                entry.getValue().setVisibility(Boolean.TRUE.equals(requestedVisibility.get(entry.getKey()))
                    ? View.VISIBLE : View.GONE);
            }
            Log.i(TAG, "All platform views resumed");
        });
    }

    /**
     * Dispose all platform views. Called during activity cleanup.
     */
    public static void disposeAll() {
        if (Looper.myLooper() == Looper.getMainLooper()) disposeAllOnMain();
        else if (!mainHandler.post(GpuiPlatformView::disposeAllOnMain))
            Log.e(TAG, "UI queue unavailable for platform view cleanup");
    }

    private static void disposeAllOnMain() {
            for (FrameLayout container : containers.values()) {
                if (rootContainer != null) {
                    rootContainer.removeView(container);
                }
                container.removeAllViews();
            }
            for (View view : views.values()) destroyNative(view);
            views.clear();
            containers.clear();
            requestedVisibility.clear();
            paused = false;
            if (rootContainer != null) {
                android.view.ViewParent parent = rootContainer.getParent();
                if (parent instanceof ViewGroup) ((ViewGroup) parent).removeView(rootContainer);
                rootContainer = null;
            }
            Log.i(TAG, "All platform views disposed");
    }

    /**
     * Check if a touch point hits any visible platform view.
     *
     * Coordinates are in physical pixels relative to the window.
     * This can be called from any thread (reads only from containers map
     * on the calling thread, but note that container positions are set
     * asynchronously on the UI thread, so there is a small race window).
     *
     * For the primary hit-test path, the Rust side uses its own
     * PlatformViewRegistry.hit_test() which is synchronous and lock-free
     * on the native thread. This Java method is provided as a convenience
     * for Java-side callers.
     *
     * @param x Physical pixel x coordinate
     * @param y Physical pixel y coordinate
     * @return true if the point falls within a visible platform view
     */
    public static boolean hitTest(float x, float y) {
        for (Map.Entry<Long, FrameLayout> entry : containers.entrySet()) {
            FrameLayout container = entry.getValue();
            if (container.getVisibility() != View.VISIBLE) continue;

            int[] location = new int[2];
            container.getLocationOnScreen(location);
            int left = location[0];
            int top = location[1];
            int right = left + container.getWidth();
            int bottom = top + container.getHeight();

            if (x >= left && x <= right && y >= top && y <= bottom) {
                return true;
            }
        }
        return false;
    }

    /**
     * Dispatch a touch event to the platform view hierarchy.
     *
     * Used to forward NativeActivity input events to platform views
     * when the Rust-side hit-test determines the touch lands on a
     * platform view. The event is posted to the UI thread for dispatch.
     *
     * @param x      Physical pixel x coordinate
     * @param y      Physical pixel y coordinate
     * @param action MotionEvent action constant (0=DOWN, 1=UP, 2=MOVE)
     */
    public static void dispatchTouch(float x, float y, int action) {
        mainHandler.post(() -> {
            if (rootContainer == null) return;

            long now = android.os.SystemClock.uptimeMillis();
            android.view.MotionEvent event = android.view.MotionEvent.obtain(
                now, now, action, x, y, 0
            );
            rootContainer.dispatchTouchEvent(event);
            event.recycle();
        });
    }

    /**
     * Log the view hierarchy from a root view for debugging.
     */
    private static void logViewHierarchy(View view, int depth) {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < depth; i++) sb.append("  ");
        sb.append(view.getClass().getSimpleName());
        sb.append(" [").append(view.getWidth()).append("x").append(view.getHeight()).append("]");
        sb.append(" vis=").append(view.getVisibility() == View.VISIBLE ? "V" : view.getVisibility() == View.GONE ? "G" : "I");
        if (view instanceof android.view.SurfaceView) {
            sb.append(" (SurfaceView)");
        }
        Log.i(TAG, sb.toString());
        if (view instanceof ViewGroup) {
            ViewGroup vg = (ViewGroup) view;
            for (int i = 0; i < vg.getChildCount(); i++) {
                logViewHierarchy(vg.getChildAt(i), depth + 1);
            }
        }
    }
}
