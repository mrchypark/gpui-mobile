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
    private static native void composingRegion(long session, long editor, String text,
                                                int startOffset, int endOffset);
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

    /** Restart only the still-current native editor after a rejected IME region. */
    public static void restart(Activity activity, long session, long editorEpoch) {
        ViewGroup content = activity.findViewById(android.R.id.content);
        GpuiKeyboard editor = content == null ? null : content.findViewWithTag(TAG);
        if (editor == null || editor.session != session || !editor.hasFocus()
            || editorEpoch() != editorEpoch || !hasLogicalOwner()) return;
        InputMethodManager manager = activity.getSystemService(InputMethodManager.class);
        if (manager != null) manager.restartInput(editor);
    }

    @Override public boolean onCheckIsTextEditor() { return true; }

    @Override public InputConnection onCreateInputConnection(EditorInfo info) {
        android.util.Log.i(TAG, "IME create session=" + session + " focused=" + hasFocus());
        info.inputType = inputType;
        info.imeOptions = imeOptions;
        // This proxy starts empty. Do not leave the IME's initial cursor unknown.
        info.initialSelStart = 0;
        info.initialSelEnd = 0;
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
            private void publishSelection() {
                Editable text = getEditable();
                int start = android.text.Selection.getSelectionStart(text);
                int end = android.text.Selection.getSelectionEnd(text);
                if (start < 0 || end < 0) {
                    start = end = text.length();
                    android.text.Selection.setSelection(text, start);
                }
                int composingStart = BaseInputConnection.getComposingSpanStart(text);
                int composingEnd = BaseInputConnection.getComposingSpanEnd(text);
                if (composingStart < 0 || composingEnd < composingStart) {
                    composingStart = composingEnd = -1;
                }
                InputMethodManager manager = getContext().getSystemService(InputMethodManager.class);
                if (manager != null) {
                    manager.updateSelection(GpuiKeyboard.this, start, end, composingStart, composingEnd);
                }
            }
            @Override public boolean setComposingRegion(int start, int end) {
                if (!current()) return false;
                Editable text = getEditable();
                int cursor = android.text.Selection.getSelectionStart(text);
                int regionStart = Math.min(start, end);
                int regionEnd = Math.max(start, end);
                if (regionStart < 0 || regionEnd > text.length()
                    || cursor < 0 || android.text.Selection.getSelectionEnd(text) != cursor)
                    return false;
                boolean result = super.setComposingRegion(regionStart, regionEnd);
                if (result && logicalOwner) {
                    // Anchor the region at the current caret. Native verifies this exact text
                    // against its focused handler before the next composing replacement.
                    composingRegion(connectionSession, connectionEditor,
                        text.subSequence(regionStart, regionEnd).toString(),
                        regionStart - cursor, regionEnd - cursor);
                }
                if (result) publishSelection();
                return result;
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
                if (result) {
                    if (logicalOwner) publishComposition();
                    publishSelection();
                }
                return result;
            }
            private void publishComposition() {
                Editable editable = getEditable();
                int composingStart = BaseInputConnection.getComposingSpanStart(editable);
                int composingEnd = BaseInputConnection.getComposingSpanEnd(editable);
                if (composingStart < 0 || composingEnd < composingStart
                    || composingEnd > editable.length()) return;
                int start = android.text.Selection.getSelectionStart(editable) - composingStart;
                int end = android.text.Selection.getSelectionEnd(editable) - composingStart;
                int length = composingEnd - composingStart;
                start = Math.max(0, Math.min(start, length));
                end = Math.max(0, Math.min(end, length));
                composed(connectionSession, connectionEditor,
                    editable.subSequence(composingStart, composingEnd).toString(), start, end);
            }
            @Override public boolean commitText(CharSequence text, int cursor) {
                android.util.Log.i(TAG, "IME commit current=" + current()
                    + " epoch=" + editorEpoch() + " compositionEpoch=" + compositionEditor
                    + " composing=" + composing);
                if (!current()) return false;
                long editor = editorEpoch();
                boolean stale = composing && compositionEditor != editor;
                if (stale) return false;
                boolean result = super.commitText(text, cursor);
                if (!result) return false;
                composing = false;
                publishSelection();
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
                if (logicalOwner) {
                    boolean result = super.finishComposingText();
                    if (result) publishSelection();
                    return result;
                }
                text.clear();
                publishSelection();
                return true;
            }
            @Override public boolean deleteSurroundingText(int before, int after) {
                if (!current()) return false;
                if (composing || BaseInputConnection.getComposingSpanStart(getEditable()) >= 0) return false;
                if (before < 0 || after != 0) return false;
                boolean accepted = deleteBefore(connectionSession, editorEpoch(), before, false);
                if (accepted) {
                    super.deleteSurroundingText(before, after);
                    publishSelection();
                }
                return accepted;
            }
            @Override public boolean deleteSurroundingTextInCodePoints(int before, int after) {
                if (!current()) return false;
                if (composing || BaseInputConnection.getComposingSpanStart(getEditable()) >= 0) return false;
                if (before < 0 || after != 0) return false;
                boolean accepted = deleteBefore(connectionSession, editorEpoch(), before, true);
                if (accepted) {
                    super.deleteSurroundingTextInCodePoints(before, after);
                    publishSelection();
                }
                return accepted;
            }
            @Override public boolean performEditorAction(int action) {
                if (!current()) return false;
                committed(connectionSession, editorEpoch(), 1, "");
                return true;
            }
            @Override public boolean sendKeyEvent(KeyEvent event) {
                if (!current()) return false;
                if (event.getAction() != KeyEvent.ACTION_DOWN) return true;
                if (event.getKeyCode() == KeyEvent.KEYCODE_DEL) {
                    // Native key handling can change text or focus asynchronously.
                    getEditable().clear();
                    composing = false;
                    publishSelection();
                    committed(connectionSession, editorEpoch(), 2, "");
                }
                else if (event.getKeyCode() == KeyEvent.KEYCODE_ENTER) {
                    getEditable().clear();
                    composing = false;
                    publishSelection();
                    committed(connectionSession, editorEpoch(), 1, "");
                }
                else {
                    int unicode = event.getUnicodeChar();
                    // Dead-key values carry a flag, not a Unicode scalar. Let the IME
                    // handle unsupported dead keys rather than claiming a committed accent.
                    if ((unicode & android.view.KeyCharacterMap.COMBINING_ACCENT) != 0)
                        return false;
                    if (unicode == 0) return super.sendKeyEvent(event);
                    String text = new String(Character.toChars(unicode));
                    if (!super.commitText(text, 1)) return false;
                    composing = false;
                    publishSelection();
                    committed(connectionSession, editorEpoch(), 0, text);
                }
                return true;
            }
        };
    }
}
