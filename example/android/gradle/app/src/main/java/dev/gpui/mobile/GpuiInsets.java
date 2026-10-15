package dev.gpui.mobile;

import android.app.Activity;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewTreeObserver;
import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowInsetsCompat;
import java.util.Arrays;

/** Observes OS geometry without consuming insets or changing app layout. */
public final class GpuiInsets implements View.OnAttachStateChangeListener,
        ViewTreeObserver.OnGlobalLayoutListener {
    private static final String TAG = "dev.gpui.mobile.insets-observer";
    private final View root;
    private final View probe;
    private int[] previous;
    private float previousDensity;

    public static void install(Activity activity) {
        ViewGroup content = activity.findViewById(android.R.id.content);
        if (content == null || content.findViewWithTag(TAG) != null) return;
        new GpuiInsets(activity, content);
    }

    private GpuiInsets(Activity activity, ViewGroup content) {
        root = activity.getWindow().getDecorView();
        probe = new View(activity);
        probe.setTag(TAG);
        probe.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        probe.setClickable(false);
        probe.setFocusable(false);
        probe.addOnAttachStateChangeListener(this);
        ViewCompat.setOnApplyWindowInsetsListener(probe, (view, insets) -> {
            publish();
            return insets;
        });
        // A zero-size observer does not intercept input or alter NativeActivity's surface.
        content.addView(probe, new ViewGroup.LayoutParams(0, 0));
    }

    @Override public void onViewAttachedToWindow(View view) {
        root.getViewTreeObserver().addOnGlobalLayoutListener(this);
        ViewCompat.requestApplyInsets(probe);
        publish();
    }

    @Override public void onViewDetachedFromWindow(View view) {
        if (root.getViewTreeObserver().isAlive()) {
            root.getViewTreeObserver().removeOnGlobalLayoutListener(this);
        }
        previous = null;
    }

    @Override public void onGlobalLayout() { publish(); }

    private void publish() {
        WindowInsetsCompat insets = ViewCompat.getRootWindowInsets(root);
        View surface = findSurface(root);
        if (insets == null || surface == null || surface.getWidth() <= 0 || surface.getHeight() <= 0) return;
        int[] location = new int[2];
        surface.getLocationInWindow(location);
        int[] rootLocation = new int[2];
        root.getLocationInWindow(rootLocation);
        int x = location[0] - rootLocation[0], y = location[1] - rootLocation[1];
        int width = surface.getWidth(), height = surface.getHeight();
        Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
        Insets ime = insets.getInsets(WindowInsetsCompat.Type.ime());
        boolean visible = insets.isVisible(WindowInsetsCompat.Type.ime());
        int[] safe = relativeEdges(bars.top, bars.bottom, bars.left, bars.right,
                root.getWidth(), root.getHeight(), x, y, width, height);
        int[] keyboard = relativeEdges(ime.top, ime.bottom, ime.left, ime.right,
                root.getWidth(), root.getHeight(), x, y, width, height);
        // Android exposes edge occlusion, not the rectangle of a floating keyboard.
        int[] rect = visible && ime.bottom > 0
                ? intersection(0, root.getHeight() - ime.bottom, root.getWidth(), ime.bottom,
                        x, y, width, height) : new int[4];
        int[] snapshot = {safe[0], safe[1], safe[2], safe[3], keyboard[0], keyboard[1],
                keyboard[2], keyboard[3], visible ? 1 : 0, rect[0], rect[1], rect[2], rect[3]};
        float density = root.getResources().getDisplayMetrics().density;
        if (Arrays.equals(previous, snapshot) && density == previousDensity) return;
        nativeInsets(density, safe[0], safe[1], safe[2], safe[3],
                keyboard[0], keyboard[1], keyboard[2], keyboard[3], visible,
                rect[0], rect[1], rect[2], rect[3]);
        previous = snapshot;
        previousDensity = density;
    }

    private static View findSurface(View view) {
        if (view instanceof SurfaceView) return view;
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                View found = findSurface(group.getChildAt(i));
                if (found != null) return found;
            }
        }
        return null;
    }

    static int[] relativeEdges(int top, int bottom, int left, int right,
            int rootWidth, int rootHeight, int x, int y, int width, int height) {
        return new int[] {clamp(top - y, height), clamp(y + height - (rootHeight - bottom), height),
                clamp(left - x, width), clamp(x + width - (rootWidth - right), width)};
    }

    static int[] intersection(int x, int y, int width, int height,
            int surfaceX, int surfaceY, int surfaceWidth, int surfaceHeight) {
        int left = Math.max(x, surfaceX), top = Math.max(y, surfaceY);
        int right = Math.min(x + width, surfaceX + surfaceWidth);
        int bottom = Math.min(y + height, surfaceY + surfaceHeight);
        if (right <= left || bottom <= top) return new int[4];
        return new int[] {left - surfaceX, top - surfaceY, right - left, bottom - top};
    }

    private static int clamp(int value, int maximum) { return Math.max(0, Math.min(value, maximum)); }

    private static native void nativeInsets(float density, int top, int bottom, int left, int right,
            int imeTop, int imeBottom, int imeLeft, int imeRight, boolean imeVisible,
            int keyboardX, int keyboardY, int keyboardWidth, int keyboardHeight);
}
