package dev.gpui.mobile;

import android.app.Activity;
import android.app.AlertDialog;
import android.widget.ArrayAdapter;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.TextView;

/** Native prompts, invoked on the Activity main thread. */
public final class GpuiPrompt {
    private static native void complete(long request, int answer);

    public static void show(Activity activity, long request, String message,
            String detail, String[] answers, int cancel) {
        if (activity.isFinishing() || activity.isDestroyed()) {
            complete(request, -1);
            return;
        }
        LinearLayout content = new LinearLayout(activity);
        content.setOrientation(LinearLayout.VERTICAL);
        TextView description = new TextView(activity);
        description.setText(detail);
        content.addView(description);
        ListView choices = new ListView(activity);
        choices.setAdapter(new ArrayAdapter<>(activity, android.R.layout.simple_list_item_1, answers));
        content.addView(choices);
        AlertDialog dialog = new AlertDialog.Builder(activity).setTitle(message)
                .setView(content).create();
        android.view.View decor = activity.getWindow().getDecorView();
        android.view.View.OnAttachStateChangeListener lifetime = new android.view.View.OnAttachStateChangeListener() {
            @Override public void onViewAttachedToWindow(android.view.View view) {}
            @Override public void onViewDetachedFromWindow(android.view.View view) {
                complete(request, -1);
                dialog.dismiss();
            }
        };
        decor.addOnAttachStateChangeListener(lifetime);
        choices.setOnItemClickListener((parent, view, position, id) -> {
            complete(request, position);
            dialog.dismiss();
        });
        dialog.setOnCancelListener(ignored -> complete(request, cancel));
        // Removal is idempotent, including Activity teardown after a selection.
        dialog.setOnDismissListener(ignored -> {
            decor.removeOnAttachStateChangeListener(lifetime);
            complete(request, -1);
        });
        try { dialog.show(); }
        catch (RuntimeException error) {
            decor.removeOnAttachStateChangeListener(lifetime);
            complete(request, -1);
            throw error;
        }
    }
}
