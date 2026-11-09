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
    private static String region = "";
    private static int markedStart, markedEnd;
    private static int regionStart, regionEnd;
    private static int deletes, compositions, lastAction = -1;
    private static EditorInfo lastInfo;
    public static void recordCommit(long session, long editor, int action, String text) {
        lastAction = action;
        if (action == 0) committed += text;
    }
    public static void recordComposition(long session, long editor, String text, int start, int end) {
        marked = text; markedStart = start; markedEnd = end; compositions++;
    }
    public static void recordCompositionRegion(long session, long editor, String text, int start, int end) {
        region = text; regionStart = start; regionEnd = end;
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
        lastInfo = new EditorInfo();
        return keyboard.onCreateInputConnection(lastInfo);
    }
    private static void exercise(Activity host) throws Exception {
        InputConnection keys = connection(host);
        check(lastInfo.initialSelStart == 0 && lastInfo.initialSelEnd == 0,
            "connection publishes an initial proxy selection");
        check(!keys.sendKeyEvent(unicode(KeyCharacterMap.COMBINING_ACCENT | '`')), "dead key declined");
        check(committed.isEmpty(), "dead key must not commit a flagged scalar");
        check(keys.sendKeyEvent(unicode('a')), "ordinary letter accepted");
        check(keys.sendKeyEvent(unicode(0x1F980)), "supplementary scalar accepted");
        check(committed.equals("a🦀"), "exact letter/supplementary commits");
        check(((BaseInputConnection)keys).getEditable().toString().equals("a🦀"),
            "hardware scalars retain proxy context");
        keys.closeConnection();
        InputConnection hardware = connection(host);
        check(hardware.commitText("gpu", 1), "hardware invalidation source text");
        check(hardware.sendKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_DEL)),
            "hardware backspace accepted");
        check(lastAction == 2 && ((BaseInputConnection)hardware).getEditable().length() == 0,
            "hardware backspace invalidates proxy context");
        check(hardware.commitText("gpu", 1), "hardware enter source text");
        check(hardware.sendKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_ENTER)),
            "hardware enter accepted");
        check(lastAction == 1 && ((BaseInputConnection)hardware).getEditable().length() == 0,
            "hardware enter invalidates proxy context");
        hardware.closeConnection();
        for (boolean codepoints : new boolean[]{false, true}) {
            InputConnection input = connection(host);
            committed = "";
            marked = "";
            check(input.commitText("A", 1), "committed prefix");
            check(input.setComposingText("ㅎ", 1), "initial Hangul composition");
            check(marked.equals("ㅎ"), "initial jamo reaches native composition");
            check(input.setComposingText("하", 1), "complete Hangul syllable composition");
            check(marked.equals("하"), "complete syllable replaces jamo composition");
            check(input.setComposingText("각", 1), "final-consonant composition");
            check(marked.equals("각"), "final consonant stays in the marked composition");
            check(input.setComposingText("하", 1), "IME composing backspace");
            check(marked.equals("하"), "composing backspace replaces the complete syllable");
            check(input.setComposingText("한", 1), "Hangul replacement");
            check(marked.equals("한"), "composition replaces instead of appending");
            Editable proxy = ((BaseInputConnection)input).getEditable();
            check(proxy.toString().equals("A한"), "proxy retains committed context and composition");
            check(BaseInputConnection.getComposingSpanStart(proxy) == 1
                && BaseInputConnection.getComposingSpanEnd(proxy) == 2, "real framework composing span");
            int beforeDeletes = deletes, beforeCompositions = compositions;
            boolean result = codepoints ? input.deleteSurroundingTextInCodePoints(1, 0)
                                        : input.deleteSurroundingText(1, 0);
            check(!result, "unsupported composing-context deletion must not acknowledge success");
            check(committed.equals("A") && marked.equals("한") && proxy.toString().equals("A한"),
                "rejected deletion preserves prefix and mark");
            check(deletes == beforeDeletes && compositions == beforeCompositions, "no misleading native publication");
            check(input.commitText("한", 1), "composition commit still works");
            check(committed.equals("A한"), "exact committed document");
            check(codepoints ? input.deleteSurroundingTextInCodePoints(1, 0)
                             : input.deleteSurroundingText(1, 0), "ordinary native deletion delegated");
            check(deletes == beforeDeletes + 1, "ordinary deletion dispatched once");
            input.closeConnection();
        }
        InputConnection prefixed = connection(host);
        Editable proxy = ((BaseInputConnection)prefixed).getEditable();
        proxy.append("gpu");
        android.text.Selection.setSelection(proxy, proxy.length());
        marked = "";
        check(prefixed.setComposingText("한", 1), "prefixed Hangul composition");
        check(marked.equals("한"), "composition excludes retained proxy prefix");
        check(markedStart == 1 && markedEnd == 1, "composition selection is relative to its span");
        prefixed.closeConnection();
        InputConnection samsung = connection(host);
        Editable samsungProxy = ((BaseInputConnection)samsung).getEditable();
        check(samsung.setComposingText("gpu", 1), "English composition established");
        check(samsung.finishComposingText(), "finish preserves Samsung proxy context");
        check(samsungProxy.toString().equals("gpu"), "finished proxy text retained for region");
        check(samsung.setComposingRegion(3, 0), "reversed Samsung composing region accepted");
        check(region.equals("gpu") && regionStart == -3 && regionEnd == 0,
            "region carries exact prior text relative to caret");
        check(samsung.setComposingText("gpuㅎ", 1), "Samsung full-range Korean composition");
        check(marked.equals("gpuㅎ") && markedStart == 4 && markedEnd == 4,
            "full composing buffer remains explicit for native replacement");
        samsung.closeConnection();
        InputConnection directCommit = connection(host);
        check(directCommit.setComposingText("gpu", 1), "direct commit source composition");
        check(directCommit.finishComposingText(), "direct commit source finish");
        check(directCommit.setComposingRegion(0, 3), "direct commit region accepted");
        committed = "";
        check(directCommit.commitText("gpu한", 1), "direct full-region commit accepted");
        check(committed.equals("gpu한"), "direct commit preserves full replacement payload");
        directCommit.closeConnection();
        InputConnection gboard = connection(host);
        Editable gboardProxy = ((BaseInputConnection)gboard).getEditable();
        committed = "";
        check(gboard.commitText("ㅎ", 1), "first committed jamo accepted");
        check(gboard.commitText("ㅏ", 1), "second committed jamo accepted");
        check(gboardProxy.toString().equals("ㅎㅏ"), "committed jamo remain in proxy context");
        check(gboard.getTextBeforeCursor(2, 0).toString().equals("ㅎㅏ"),
            "IME can read committed jamo context");
        check(committed.equals("ㅎㅏ"), "native receives committed jamo without synthesis");
        gboard.closeConnection();
        for (boolean codepoints : new boolean[]{false, true}) {
            InputConnection finished = connection(host);
            Editable finishedProxy = ((BaseInputConnection)finished).getEditable();
            check(finished.setComposingText("gpu", 1), "finished deletion source composition");
            check(finished.finishComposingText(), "finished deletion source finish");
            int beforeDeletes = deletes;
            boolean accepted = codepoints ? finished.deleteSurroundingTextInCodePoints(1, 0)
                : finished.deleteSurroundingText(1, 0);
            check(accepted, "post-finish deletion delegated");
            check(deletes == beforeDeletes + 1 && finishedProxy.toString().equals("gp"),
                "accepted post-finish deletion keeps proxy context synchronized");
            finished.closeConnection();
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
