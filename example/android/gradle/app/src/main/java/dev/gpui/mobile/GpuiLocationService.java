package dev.gpui.mobile;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.IBinder;
import java.util.HashSet;

/** Location-only, main-thread leases. No automatic restart or BLE ownership. */
public final class GpuiLocationService extends Service {
    private static final HashSet<Long> OWNERS = new HashSet<>();
    private static GpuiLocationService current;
    static final class Lifecycle {
        private Object owner;
        private boolean stopping;
        boolean ready() { return owner != null && !stopping; }
        boolean canAcquire() { return !stopping; }
        void started(Object service) { owner = service; }
        void stopping() { stopping = true; }
        boolean destroyed(Object service) {
            if (owner != service) return false;
            owner = null;
            stopping = false;
            return true;
        }
    }
    private static final Lifecycle LIFECYCLE = new Lifecycle();
    private static final String CHANNEL = "gpui.location.tracking";
    private static final int NOTIFICATION = 0x475053;
    private static final String STOP = "dev.gpui.mobile.location.STOP";
    static boolean isReady() { return LIFECYCLE.ready(); }
    static void acquire(Context context, long id) {
        // stopSelf destruction is asynchronous. Never attach a new watch to that owner.
        if (!LIFECYCLE.canAcquire()) throw new IllegalStateException("Location service stopping; retry from the visible app");
        OWNERS.add(id);
        if (isReady()) { GpuiLocation.backgroundReady(); return; }
        try { context.startForegroundService(new Intent(context, GpuiLocationService.class)); }
        catch (RuntimeException error) { OWNERS.remove(id); throw error; }
    }
    static void release(Context context, long id) {
        OWNERS.remove(id);
        if (OWNERS.isEmpty() && current != null) {
            LIFECYCLE.stopping();
            current.stopSelf();
        }
        // A pending start still must promote to foreground, then stop in onStartCommand.
    }
    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && STOP.equals(intent.getAction())) {
            GpuiLocation.backgroundStopped("user_stopped");
            stopSelf();
            return START_NOT_STICKY;
        }
        try {
            NotificationManager manager = getSystemService(NotificationManager.class);
            manager.createNotificationChannel(new NotificationChannel(CHANNEL, "Location tracking", NotificationManager.IMPORTANCE_LOW));
            PendingIntent stop = PendingIntent.getService(this, 0,
                new Intent(this, GpuiLocationService.class).setAction(STOP), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
            Notification notification = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(android.R.drawable.ic_menu_mylocation)
                .setContentTitle("Location tracking active")
                .setContentText("The app is receiving location updates. Tap Stop to end tracking.")
                .setOngoing(true)
                .addAction(new Notification.Action.Builder(null, "Stop", stop).build()).build();
            if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_LOCATION);
            else startForeground(NOTIFICATION, notification);
            current = this;
            LIFECYCLE.started(this);
            if (OWNERS.isEmpty()) { LIFECYCLE.stopping(); stopSelf(); }
            else if (isReady()) GpuiLocation.backgroundReady();
        } catch (RuntimeException error) {
            GpuiLocation.backgroundStopped("foreground_start_failed:" + error.getClass().getSimpleName());
            stopSelf();
        }
        return START_NOT_STICKY;
    }
    @Override public void onDestroy() {
        if (LIFECYCLE.destroyed(this)) {
            current = null;
            GpuiLocation.backgroundStopped("service_destroyed");
        }
        stopForeground(STOP_FOREGROUND_REMOVE);
        super.onDestroy();
    }
    @Override public IBinder onBind(Intent intent) { return null; }
}
