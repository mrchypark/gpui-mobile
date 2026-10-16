package org.gpui.mobile.ble;

import android.app.*;
import android.content.*;
import android.content.pm.*;
import android.os.*;

/** Explicit, single-owner connected BLE service. Never scans or restarts itself. */
public final class BleConnectionService extends Service {
    private static final String CHANNEL = "gpui_ble_connection";
    private static final int NOTIFICATION = 0x424c45;
    private static long sequence, owner;
    private static int state;
    private static String failure;
    private static Context application;

    public static synchronized long start(Activity activity) throws Exception {
        if (owner != 0) throw new IllegalStateException("Another BLE background session owns the service");
        if (activity.isFinishing() || activity.isDestroyed() || !activity.hasWindowFocus())
            throw new IllegalStateException("Enable BLE background mode from a visible Activity");
        Context context = activity.getApplicationContext();
        ServiceInfo info = context.getPackageManager().getServiceInfo(
            new ComponentName(context, BleConnectionService.class), 0);
        if (info.exported) throw new IllegalStateException("BLE service must not be exported");
        if (Build.VERSION.SDK_INT >= 29 && (info.getForegroundServiceType() & ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE) == 0)
            throw new IllegalStateException("BLE service needs connectedDevice type");
        if (Build.VERSION.SDK_INT >= 28 && context.checkSelfPermission("android.permission.FOREGROUND_SERVICE") != PackageManager.PERMISSION_GRANTED)
            throw new SecurityException("Declare FOREGROUND_SERVICE");
        if (Build.VERSION.SDK_INT >= 34 && context.checkSelfPermission("android.permission.FOREGROUND_SERVICE_CONNECTED_DEVICE") != PackageManager.PERMISSION_GRANTED)
            throw new SecurityException("Declare FOREGROUND_SERVICE_CONNECTED_DEVICE");
        if (Build.VERSION.SDK_INT >= 31 && context.checkSelfPermission("android.permission.BLUETOOTH_CONNECT") != PackageManager.PERMISSION_GRANTED)
            throw new SecurityException("Bluetooth connect permission is required");
        long token = ++sequence;
        owner = token; state = 0; failure = null; application = context;
        Intent intent = new Intent(context, BleConnectionService.class).putExtra("owner", token);
        try {
            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent);
            else context.startService(intent);
        } catch (RuntimeException error) { owner = 0; state = -1; application = null; throw error; }
        return token;
    }
    public static synchronized int status(long token) {
        if (owner != token || token == 0) throw new IllegalStateException("BLE background service ended");
        if (state < 0) throw new IllegalStateException(failure == null ? "BLE background service failed" : failure);
        if (Build.VERSION.SDK_INT >= 31 && application.checkSelfPermission("android.permission.BLUETOOTH_CONNECT") != PackageManager.PERMISSION_GRANTED)
            throw new SecurityException("Bluetooth permission revoked");
        return state;
    }
    public static synchronized void stop(long token) {
        if (owner != token || token == 0) return;
        Context context = application;
        // Invalidate before asynchronous destruction so a delayed start cannot revive this lease.
        owner = 0; state = -1; application = null;
        if (context != null) context.stopService(new Intent(context, BleConnectionService.class));
    }
    private long token;
    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        synchronized (BleConnectionService.class) {
            token = intent == null ? 0 : intent.getLongExtra("owner", 0);
            if (token == 0 || token != owner) { stopSelf(startId); return START_NOT_STICKY; }
            try {
                NotificationManager manager = (NotificationManager)getSystemService(NOTIFICATION_SERVICE);
                if (Build.VERSION.SDK_INT >= 26) manager.createNotificationChannel(
                    new NotificationChannel(CHANNEL, "Bluetooth connection", NotificationManager.IMPORTANCE_LOW));
                Notification.Builder builder = Build.VERSION.SDK_INT >= 26 ? new Notification.Builder(this, CHANNEL) : new Notification.Builder(this);
                Notification notification = builder.setSmallIcon(android.R.drawable.stat_sys_data_bluetooth)
                    .setContentTitle("Bluetooth connection active")
                    .setContentText("An explicitly selected BLE device remains connected.")
                    .setOngoing(true).build();
                if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE);
                else startForeground(NOTIFICATION, notification);
                state = 1;
            } catch (RuntimeException error) {
                failure = "Unable to start BLE connected-device foreground service";
                state = -1; stopSelf(startId);
            }
        }
        return START_NOT_STICKY;
    }
    @Override public void onDestroy() {
        synchronized (BleConnectionService.class) {
            if (owner == token) { state = -1; failure = "BLE background service destroyed"; }
        }
        super.onDestroy();
    }
    @Override public IBinder onBind(Intent intent) { return null; }
}
