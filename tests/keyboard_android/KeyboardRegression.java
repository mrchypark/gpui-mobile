package dev.gpui.mobile;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Intent;
import android.os.Bundle;
import android.text.Editable;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;

/** Production connection and real Android framework; only JNI delivery is recorded. */
public final class KeyboardRegression extends Instrumentation {
    public static final class Host extends Activity {}
    private static String committed = "", marked = "";
    private static int deletes, compositions;
    public static void recordCommit(long session, long editor, int action, String text) {
        if (action == 0) committed += text;
    }
    public static void recordComposition(long session, long editor, String text, int start, int end) {
        marked = text; compositions++;
    }
    public static boolean recordDelete(long session, long editor, int count, boolean codepoints) {
        deletes++; return true;
    }
    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
    private static KeyEvent unicode(int scalar) {
        return new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_UNKNOWN) {
            @Override public int getUnicodeChar() { return scalar; }
        };
    }
    private static InputConnection connection(Activity host) throws Exception {
        Constructor<GpuiKeyboard> ctor = GpuiKeyboard.class.getDeclaredConstructor(Activity.class);
        ctor.setAccessible(true);
        GpuiKeyboard keyboard = ctor.newInstance(host);
        Field session = GpuiKeyboard.class.getDeclaredField("session");
        session.setAccessible(true);
        session.setLong(keyboard, 1);
        return keyboard.onCreateInputConnection(new EditorInfo());
    }
    private static void exercise(Activity host) throws Exception {
        InputConnection keys = connection(host);
        check(!keys.sendKeyEvent(unicode(KeyCharacterMap.COMBINING_ACCENT | '`')), "dead key declined");
        check(committed.isEmpty(), "dead key must not commit a flagged scalar");
        check(keys.sendKeyEvent(unicode('a')), "ordinary letter accepted");
        check(keys.sendKeyEvent(unicode(0x1F980)), "supplementary scalar accepted");
        check(committed.equals("a🦀"), "exact letter/supplementary commits");
        keys.closeConnection();
        for (boolean codepoints : new boolean[]{false, true}) {
            InputConnection input = connection(host);
            committed = "";
            check(input.commitText("A", 1), "committed prefix");
            check(input.setComposingText("ㅎ", 1), "initial Hangul composition");
            check(input.setComposingText("한", 1), "Hangul replacement");
            check(marked.equals("한"), "composition replaces instead of appending");
            Editable proxy = ((BaseInputConnection)input).getEditable();
            check(proxy.toString().equals("한"), "proxy holds only composing text");
            check(BaseInputConnection.getComposingSpanStart(proxy) == 0
                && BaseInputConnection.getComposingSpanEnd(proxy) == 1, "real framework composing span");
            int beforeDeletes = deletes, beforeCompositions = compositions;
            boolean result = codepoints ? input.deleteSurroundingTextInCodePoints(1, 0)
                                        : input.deleteSurroundingText(1, 0);
            check(!result, "unsupported composing-context deletion must not acknowledge success");
            check(committed.equals("A") && marked.equals("한") && proxy.toString().equals("한"),
                "rejected deletion preserves prefix and mark");
            check(deletes == beforeDeletes && compositions == beforeCompositions, "no misleading native publication");
            check(input.commitText("한", 1), "composition commit still works");
            check(committed.equals("A한"), "exact committed document");
            check(codepoints ? input.deleteSurroundingTextInCodePoints(1, 0)
                             : input.deleteSurroundingText(1, 0), "ordinary native deletion delegated");
            check(deletes == beforeDeletes + 1, "ordinary deletion dispatched once");
            input.closeConnection();
        }
    }
    @Override public void onCreate(Bundle arguments) { super.onCreate(arguments); start(); }
    @Override public void onStart() {
        Bundle result = new Bundle();
        Activity host = null;
        int status = Activity.RESULT_CANCELED;
        try {
            System.loadLibrary("gpui_keyboard_regression");
            host = startActivitySync(new Intent(getTargetContext(), Host.class).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            Activity activity = host;
            final Throwable[] failure = {null};
            runOnMainSync(() -> { try { exercise(activity); } catch (Throwable error) { failure[0] = error; } });
            if (failure[0] != null) throw new AssertionError("production connection regression", failure[0]);
            result.putString("stream", "PASS production dead-key/scalar and both composing-surrounding deletion paths\n");
            status = Activity.RESULT_OK;
        } catch (Throwable error) {
            result.putString("stream", android.util.Log.getStackTraceString(error));
        } finally {
            if (host != null) { Activity activity = host; runOnMainSync(activity::finish); }
        }
        finish(status, result);
    }
}
