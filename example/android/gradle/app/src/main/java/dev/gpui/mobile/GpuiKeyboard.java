package dev.gpui.mobile;

import android.app.Activity;
import android.text.Editable;
import android.text.InputType;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;

/** Main-thread IME proxy; committed text is queued for the native GPUI thread. */
public final class GpuiKeyboard extends View {
    private static final String TAG = "dev.gpui.mobile.keyboard";
    private int inputType;
    private int imeOptions;
    private long session;
    private final View previousFocus;
    private static native void committed(long session, long editor, int action, String text);
    private static native void composed(long session, long editor, String text, int start, int end);
    private static native boolean hasLogicalOwner();
    private static native boolean deleteBefore(long session, long editor, int count, boolean codepoints);
    private static native long editorEpoch();

    private GpuiKeyboard(Activity activity) {
        super(activity);
        previousFocus = activity.getCurrentFocus();
        setTag(TAG);
        setFocusableInTouchMode(true);
        setImportantForAccessibility(IMPORTANT_FOR_ACCESSIBILITY_NO);
    }

    public static void show(Activity activity, int inputType, int imeOptions, long session) {
        ViewGroup content = activity.findViewById(android.R.id.content);
        if (content == null || activity.isDestroyed() || activity.isFinishing()) return;
        GpuiKeyboard editor = content.findViewWithTag(TAG);
        if (editor == null) {
            editor = new GpuiKeyboard(activity);
            content.addView(editor, new ViewGroup.LayoutParams(1, 1));
        }
        editor.inputType = inputType;
        editor.imeOptions = imeOptions;
        editor.session = session;
        editor.requestFocus();
        InputMethodManager manager = activity.getSystemService(InputMethodManager.class);
        if (manager != null) {
            manager.restartInput(editor);
            manager.showSoftInput(editor, InputMethodManager.SHOW_IMPLICIT);
        }
    }

    public static void hide(Activity activity) {
        ViewGroup content = activity.findViewById(android.R.id.content);
        GpuiKeyboard editor = content == null ? null : content.findViewWithTag(TAG);
        if (editor == null) return;
        editor.session = 0;
        InputMethodManager manager = activity.getSystemService(InputMethodManager.class);
        if (manager != null) manager.hideSoftInputFromWindow(editor.getWindowToken(), 0);
        editor.clearFocus();
        if (editor.previousFocus != null && editor.previousFocus.isAttachedToWindow())
            editor.previousFocus.requestFocus();
    }

    @Override public boolean onCheckIsTextEditor() { return true; }

    @Override public InputConnection onCreateInputConnection(EditorInfo info) {
        android.util.Log.i(TAG, "IME create session=" + session + " focused=" + hasFocus());
        info.inputType = inputType;
        info.imeOptions = imeOptions;
        final long connectionSession = session;
        final long connectionEditor = editorEpoch();
        final boolean logicalOwner = hasLogicalOwner();
        return new BaseInputConnection(this, true) {
            private boolean closed;
            private long compositionEditor;
            private boolean composing;
            private boolean current() {
                return !closed && connectionSession != 0 && session == connectionSession
                    && (!logicalOwner || connectionEditor == editorEpoch());
            }
            @Override public void closeConnection() {
                android.util.Log.i(TAG, "IME close session=" + connectionSession);
                // Base close calls finishComposingText: teardown must not commit into a new editor.
                closed = true;
                getEditable().clear();
                super.closeConnection();
            }
            @Override public boolean setComposingText(CharSequence text, int cursor) {
                android.util.Log.i(TAG, "IME compose current=" + current()
                    + " epoch=" + editorEpoch() + " composing=" + composing);
                if (!current()) return false;
                long editor = editorEpoch();
                if (composing && compositionEditor != editor) {
                    getEditable().clear();
                    composing = false;
                    return false;
                }
                compositionEditor = editor;
                composing = true;
                boolean result = super.setComposingText(text, cursor);
                if (result && logicalOwner) publishComposition();
                return result;
            }
            private void publishComposition() {
                Editable editable = getEditable();
                int start = android.text.Selection.getSelectionStart(editable);
                int end = android.text.Selection.getSelectionEnd(editable);
                if (start < 0 || end < start) start = end = editable.length();
                composed(connectionSession, connectionEditor, editable.toString(), start, end);
            }
            @Override public boolean commitText(CharSequence text, int cursor) {
                android.util.Log.i(TAG, "IME commit current=" + current()
                    + " epoch=" + editorEpoch() + " compositionEpoch=" + compositionEditor
                    + " composing=" + composing);
                if (!current()) return false;
                long editor = editorEpoch();
                boolean stale = composing && compositionEditor != editor;
                composing = false;
                getEditable().clear();
                if (stale) return false;
                committed(connectionSession, editor, 0, text.toString());
                return true;
            }
            @Override public boolean finishComposingText() {
                android.util.Log.i(TAG, "IME finish current=" + current()
                    + " epoch=" + editorEpoch() + " compositionEpoch=" + compositionEditor
                    + " composing=" + composing);
                if (!current()) return false;
                Editable text = getEditable();
                long editor = editorEpoch();
                if (composing && compositionEditor == editor) {
                    if (logicalOwner) committed(connectionSession, connectionEditor, 4, "");
                    else if (text.length() > 0) committed(connectionSession, editor, 0, text.toString());
                }
                composing = false;
                text.clear();
                return true;
            }
            @Override public boolean deleteSurroundingText(int before, int after) {
                if (!current()) return false;
                if (getEditable().length() > 0) {
                    boolean result = super.deleteSurroundingText(before, after);
                    if (result && logicalOwner) publishComposition();
                    return result;
                }
                if (before < 0 || after != 0) return false;
                return deleteBefore(connectionSession, editorEpoch(), before, false);
            }
            @Override public boolean deleteSurroundingTextInCodePoints(int before, int after) {
                if (!current()) return false;
                if (getEditable().length() > 0) {
                    boolean result = super.deleteSurroundingTextInCodePoints(before, after);
                    if (result && logicalOwner) publishComposition();
                    return result;
                }
                if (before < 0 || after != 0) return false;
                return deleteBefore(connectionSession, editorEpoch(), before, true);
            }
            @Override public boolean performEditorAction(int action) {
                if (!current()) return false;
                committed(connectionSession, editorEpoch(), 1, "");
                return true;
            }
            @Override public boolean sendKeyEvent(KeyEvent event) {
                if (!current()) return false;
                if (event.getAction() != KeyEvent.ACTION_DOWN) return true;
                if (event.getKeyCode() == KeyEvent.KEYCODE_DEL) committed(connectionSession, editorEpoch(), 2, "");
                else if (event.getKeyCode() == KeyEvent.KEYCODE_ENTER) committed(connectionSession, editorEpoch(), 1, "");
                else if (event.getUnicodeChar() != 0)
                    committed(connectionSession, editorEpoch(), 0, new String(Character.toChars(event.getUnicodeChar())));
                else return super.sendKeyEvent(event);
                return true;
            }
        };
    }
}
