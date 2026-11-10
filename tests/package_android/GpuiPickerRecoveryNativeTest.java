package dev.gpui.mobile;

import android.app.Activity;
import android.content.Intent;
import android.content.SharedPreferences;
import java.io.File;
import java.lang.reflect.Field;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.util.ArrayList;

/** Run with a test Activity on Android; uses only a disposable cache file and fake preferences. */
public final class GpuiPickerRecoveryNativeTest {
    private static final class Picker extends GpuiPickerActivity {
        SharedPreferences prefs;
        boolean finished;
        @Override public SharedPreferences getSharedPreferences(String name, int mode) { return prefs; }
        @Override public void finish() { finished = true; }
    }

    public static void run(Activity host) throws Exception {
        Picker picker = new Picker();
        final int[] edits = {0};
        picker.prefs = (SharedPreferences) Proxy.newProxyInstance(
                SharedPreferences.class.getClassLoader(), new Class<?>[]{SharedPreferences.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("getBoolean")) return true;
                    if (method.getName().equals("edit")) { edits[0]++; throw new AssertionError("Pending result was edited"); }
                    throw new AssertionError(method.getName());
                });
        Class<?> requestClass = Class.forName("dev.gpui.mobile.GpuiPickerActivity$Request");
        java.lang.reflect.Constructor<?> constructor = requestClass.getDeclaredConstructor(
                String.class, Intent.class, boolean.class, File.class);
        constructor.setAccessible(true);
        Object request = constructor.newInstance("file", null, true, null);
        Method saveRequest = GpuiPickerActivity.class.getDeclaredMethod("savePendingRequest", Activity.class, requestClass);
        saveRequest.setAccessible(true);
        try {
            saveRequest.invoke(null, picker, request);
            throw new AssertionError("Mismatched pending result did not block a new request");
        } catch (InvocationTargetException expected) {
            if (!(expected.getCause() instanceof IllegalStateException)) throw expected;
        }
        if (edits[0] != 0) throw new AssertionError("Pending result was changed");

        File capture = File.createTempFile("gpui-picker-recovery-test", ".jpg", host.getCacheDir());
        try {
            Field captureField = GpuiPickerActivity.class.getDeclaredField("mCaptureFile");
            captureField.setAccessible(true);
            captureField.set(picker, capture);
            SharedPreferences.Editor editor = (SharedPreferences.Editor) Proxy.newProxyInstance(
                    SharedPreferences.Editor.class.getClassLoader(), new Class<?>[]{SharedPreferences.Editor.class},
                    (proxy, method, args) -> method.getName().equals("commit") ? false : proxy);
            picker.prefs = (SharedPreferences) Proxy.newProxyInstance(
                    SharedPreferences.class.getClassLoader(), new Class<?>[]{SharedPreferences.class},
                    (proxy, method, args) -> {
                        if (method.getName().equals("edit")) return editor;
                        throw new AssertionError(method.getName());
                    });
            Method saveResult = GpuiPickerActivity.class.getDeclaredMethod("savePendingResult", ArrayList.class);
            saveResult.setAccessible(true);
            ArrayList<String> paths = new ArrayList<>();
            paths.add("/test/import.jpg");
            saveResult.invoke(picker, paths);
            if (!capture.exists()) throw new AssertionError("Failed commit deleted capture");

            Method failImport = GpuiPickerActivity.class.getDeclaredMethod("failRecoveredImport", RuntimeException.class);
            failImport.setAccessible(true);
            failImport.invoke(picker, new IllegalStateException("fixture import failure"));
            if (capture.exists() || !picker.finished) throw new AssertionError("Failed import did not clean up");
        } finally {
            if (capture.exists() && !capture.delete()) throw new AssertionError("Cannot remove test capture");
        }
    }
}
