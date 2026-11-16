package dev.gpui.mobile;

import android.app.Activity;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;

/** Guards the Java signature used by the Rust export JNI call. */
public final class GpuiFileExportContractTest {
    public static void main(String[] args) throws Exception {
        Method export = GpuiFilePicker.class.getMethod(
                "exportFile", Activity.class, String.class, String.class, byte[].class);
        if (!Modifier.isStatic(export.getModifiers())) {
            throw new AssertionError("exportFile must be static for JNI");
        }
        if (export.getReturnType() != String.class) {
            throw new AssertionError("exportFile must return a content URI or null");
        }
        Method display = GpuiFilePicker.class.getMethod(
                "displayName", Activity.class, String.class);
        if (!Modifier.isStatic(display.getModifiers())) {
            throw new AssertionError("displayName must be static for JNI");
        }
        if (display.getReturnType() != String.class) {
            throw new AssertionError("displayName must return a provider name or null");
        }
        if (GpuiFilePicker.class.getMethod(
                "getSavePath", Activity.class, String.class, String.class).getReturnType() != String.class) {
            throw new AssertionError("getSavePath remains a path-only contract");
        }
        if (!"report_2026_q1.txt".equals(GpuiPickerFiles.safeName("report/2026\\q1.txt"))) {
            throw new AssertionError("Export names must not escape the destination directory");
        }
        System.out.println("PASS: export JNI signatures and path-only save contract");
    }
}
